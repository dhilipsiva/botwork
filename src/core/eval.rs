use pest::iterators::Pair;
use std::{
    borrow::Cow,
    collections::HashMap,
    io::{self, Write},
    sync::Arc,
};

use super::{
    ast::{
        self, AccessSegment, AssignmentValue, BinaryOp, Block, Call, Definition, ElseBranch, Expr,
        ExprKind, Name, Node, Program, Statement, StatementKind, UnaryOp,
    },
    diagnostic::{CallFrame, Diagnostic, DiagnosticResult},
    grammar::{finite_float, BWErr, Literal, LiteralResult, Operate, Rule},
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod execution_contract;

#[derive(Debug)]
enum Completion {
    Normal(Literal),
    Return(Literal),
    Break,
    Continue,
}

type CompletionResult = DiagnosticResult<Completion>;
type RuntimeResult = DiagnosticResult<Literal>;

#[derive(Clone)]
enum StmtType {
    Native {
        callback: Callback,
        name: &'static str,
    },
    UserDefined(Arc<Definition>),
}

#[derive(Clone, Default)]
struct Frame {
    variables: HashMap<String, Literal>,
    statements: HashMap<String, StmtType>,
    // Definitions are not first-class values, so lexical owners remain on the stack.
    parent: Option<usize>,
}

#[derive(Clone)]
struct HandledError {
    invocation: usize,
    diagnostic: Diagnostic,
}

#[derive(Clone)]
pub struct Context {
    frames: Vec<Frame>,
    current: usize,
    calls: Vec<CallFrame>,
    handlers: Vec<HandledError>,
    #[cfg(test)]
    expression_visits: std::cell::RefCell<Vec<String>>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            frames: vec![Frame::default()],
            current: 0,
            calls: vec![],
            handlers: vec![],
            #[cfg(test)]
            expression_visits: Default::default(),
        }
    }
}

impl Context {
    fn get_variable(&self, name: &str) -> LiteralResult {
        self.get_variable_ref(name).cloned()
    }

    fn get_variable_ref(&self, name: &str) -> Result<&Literal, BWErr> {
        let mut index = Some(self.current);
        while let Some(frame_index) = index {
            let frame = &self.frames[frame_index];
            if let Some(value) = frame.variables.get(name) {
                return Ok(value);
            }
            index = frame.parent;
        }
        Err(BWErr::VariableNotDefined(name.to_owned()))
    }

    fn set_variable(&mut self, name: String, literal: Literal) -> Option<Literal> {
        self.frames[self.current].variables.insert(name, literal)
    }

    fn get_statement(&self, signature: &str) -> Option<(StmtType, usize)> {
        let mut index = Some(self.current);
        while let Some(frame_index) = index {
            let frame = &self.frames[frame_index];
            if let Some(statement) = frame.statements.get(signature) {
                return Some((statement.clone(), frame_index));
            }
            index = frame.parent;
        }
        None
    }

    fn with_invocation(
        &mut self,
        frame: Frame,
        body: impl FnOnce(&mut Self) -> CompletionResult,
    ) -> CompletionResult {
        let caller = self.current;
        self.current = self.frames.len();
        self.frames.push(frame);
        // All language outcomes, including errors, restore the dynamic caller.
        let result = body(self);
        self.frames.pop();
        self.current = caller;
        result
    }

    pub fn init_statements(&mut self) {
        self.frames[self.current]
            .statements
            .entry("log|param|".into())
            .or_insert(StmtType::Native {
                callback: log_param,
                name: "Log",
            });
    }

    fn with_call(
        &mut self,
        frame: CallFrame,
        body: impl FnOnce(&mut Self) -> RuntimeResult,
    ) -> RuntimeResult {
        self.calls.push(frame);
        let result = body(self).map_err(|error| error.capture_stack(&self.calls));
        self.calls.pop();
        result
    }
}

type Callback = fn(&Call, &mut Context) -> RuntimeResult;

fn log_param(call: &Call, context: &mut Context) -> RuntimeResult {
    let expression = call.arguments.last().ok_or_else(|| {
        BWErr::ParameterMissingError("`Log {param}` requires at least 1 parameter".into())
    })?;
    let value = evaluate_expression(expression, context)?;
    write_log(&value, &mut io::stdout().lock())?;
    Ok(value)
}

fn write_log(value: &Literal, output: &mut impl Write) -> Result<(), BWErr> {
    writeln!(output, "{value}").map_err(|error| BWErr::OutputError(error.to_string()))
}

fn invoke(call: &Call, context: &mut Context) -> RuntimeResult {
    invoke_inner(call, context).map_err(|error| error.at(&call.span).capture_stack(&context.calls))
}

fn invoke_inner(call: &Call, context: &mut Context) -> RuntimeResult {
    let (definition, owner) = context
        .get_statement(&call.signature)
        .ok_or_else(|| BWErr::StatementNotDefined(call.span.text().to_owned()))?;
    match definition {
        StmtType::Native { callback, .. } => context.with_call(
            CallFrame {
                signature: call.signature.clone(),
                call_site: call.span.clone(),
                definition_site: None,
            },
            |context| callback(call, context),
        ),
        StmtType::UserDefined(definition) => {
            if definition.parameters.len() != call.arguments.len() {
                return Err(BWErr::ParameterMissingError(
                    "The call does not match the definition's parameter count".into(),
                )
                .into());
            }
            let arguments = call
                .arguments
                .iter()
                .map(|argument| evaluate_expression(argument, context))
                .collect::<Result<Vec<_>, _>>()?;
            let frame = Frame {
                variables: definition
                    .parameters
                    .iter()
                    .zip(arguments)
                    .map(|(parameter, value)| (parameter.text.clone(), value))
                    .collect(),
                parent: Some(owner),
                ..Frame::default()
            };
            context.with_call(
                CallFrame {
                    signature: call.signature.clone(),
                    call_site: call.span.clone(),
                    definition_site: Some(definition.span.clone()),
                },
                |context| {
                    context
                        .with_invocation(frame, |context| evaluate_block(&definition.body, context))
                        .and_then(|completion| match completion {
                            Completion::Return(value) => Ok(value),
                            // Reject unconsumed loop controls at their invocation boundary.
                            completion => finish_script(completion),
                        })
                },
            )
        }
    }
}

fn evaluate_expression(expression: &Expr, context: &Context) -> RuntimeResult {
    evaluate_expression_inner(expression, context).map_err(|error| {
        error
            .at_expression(&expression.span)
            .capture_stack(&context.calls)
    })
}

fn evaluate_expression_inner(expression: &Expr, context: &Context) -> RuntimeResult {
    #[cfg(test)]
    context
        .expression_visits
        .borrow_mut()
        .push(expression.span.text().to_owned());

    match &expression.kind {
        ExprKind::Integer(text) => text
            .parse::<i32>()
            .map(Literal::Int)
            .map_err(|error| BWErr::ParsingIntegerError(error.to_string()).into()),
        ExprKind::Float(text) => {
            let value = text
                .parse::<f32>()
                .map_err(|error| BWErr::ParsingIntegerError(error.to_string()))?;
            finite_float(value, "Float literal").map_err(Into::into)
        }
        ExprKind::Bool(value) => Ok(Literal::Bool(*value)),
        ExprKind::String(value) => Ok(Literal::String(value.clone())),
        ExprKind::Variable(name) => context.get_variable(name).map_err(Into::into),
        ExprKind::Access { base, segments } => evaluate_access(base, segments, context),
        ExprKind::Array(elements) => elements
            .iter()
            .map(|element| evaluate_expression(element, context))
            .collect::<Result<Vec<_>, _>>()
            .map(Literal::Array),
        ExprKind::Map(entries) => {
            let mut values = HashMap::new();
            for (key, expression) in entries {
                values.insert(key.text.clone(), evaluate_expression(expression, context)?);
            }
            Ok(Literal::Map(values))
        }
        ExprKind::Unary {
            operator, operand, ..
        } => match (operator, &operand.kind) {
            (UnaryOp::Negate, ExprKind::Integer(text)) => {
                // Convert the signed atom together: MIN's positive magnitude is not i32.
                // Compound operands still evaluate normally before checked negation.
                #[cfg(test)]
                context
                    .expression_visits
                    .borrow_mut()
                    .push(operand.span.text().to_owned());
                format!("-{text}")
                    .parse::<i32>()
                    .map(Literal::Int)
                    .map_err(|error| BWErr::ParsingIntegerError(error.to_string()).into())
            }
            _ => operator
                .to_rule()
                .operate_unary(evaluate_expression(operand, context)?)
                .map_err(Into::into),
        },
        ExprKind::Binary {
            operator,
            left,
            right,
            ..
        } => {
            let left = evaluate_expression(left, context)?;
            if matches!(operator, BinaryOp::And | BinaryOp::Or) {
                let Literal::Bool(value) = &left else {
                    let name = if *operator == BinaryOp::And {
                        "and"
                    } else {
                        "or"
                    };
                    return Err(BWErr::OperationIncompatibleError(format!(
                        "The left operand of `{name}` must be a boolean"
                    ))
                    .into());
                };
                if (*operator == BinaryOp::And && !value) || (*operator == BinaryOp::Or && *value) {
                    return Ok(Literal::Bool(*value));
                }
            }
            let right = evaluate_expression(right, context)?;
            operator
                .to_rule()
                .operate_binary(left, right)
                .map_err(Into::into)
        }
    }
}

fn evaluate_access(base: &Expr, segments: &[AccessSegment], context: &Context) -> RuntimeResult {
    // Expressions cannot change bindings. Borrow a variable's containers throughout
    // index evaluation, or own a temporary base, then copy only the selected result.
    let container = if let ExprKind::Variable(name) = &base.kind {
        #[cfg(test)]
        context
            .expression_visits
            .borrow_mut()
            .push(base.span.text().to_owned());
        Cow::Borrowed(
            context
                .get_variable_ref(name)
                .map_err(|error| Diagnostic::new(error).at_expression(&base.span))?,
        )
    } else {
        Cow::Owned(evaluate_expression(base, context)?)
    };
    let error = |segment: &AccessSegment, reason: String| {
        let mut path = base.span.text().trim().to_owned();
        for part in segments {
            match part {
                AccessSegment::Literal(name) => {
                    path.push('.');
                    path.push_str(&name.text);
                }
                AccessSegment::Computed { span, .. } => path.push_str(span.text().trim()),
            }
        }
        let span = match segment {
            AccessSegment::Literal(name) => &name.span,
            AccessSegment::Computed { span, .. } => span,
        };
        Diagnostic::new(BWErr::CollectionAccessError {
            path,
            segment: match segment {
                AccessSegment::Literal(name) => name.text.clone(),
                AccessSegment::Computed { span, .. } => span.text().trim().to_owned(),
            },
            reason,
        })
        .at_expression(span)
    };
    let mut value = container.as_ref();
    for segment in segments {
        // Evaluate this key before checking its receiver/type; do not evaluate
        // any later key until this lookup succeeds.
        let key = match segment {
            AccessSegment::Literal(_) => None,
            AccessSegment::Computed { index, .. } => Some(evaluate_expression(index, context)?),
        };
        value = match value {
            Literal::Map(values) => {
                let name = match (segment, &key) {
                    (AccessSegment::Literal(name), _) => &name.text,
                    (_, Some(Literal::String(name))) => name,
                    _ => return Err(error(segment, "map key must be a string".into())),
                };
                values
                    .get(name)
                    .ok_or_else(|| error(segment, "map key does not exist".into()))?
            }
            Literal::Array(values) => {
                let index = match (segment, &key) {
                    (AccessSegment::Literal(name), _) => {
                        if name.text.is_empty()
                            || !name.text.bytes().all(|byte| byte.is_ascii_digit())
                        {
                            return Err(error(
                                segment,
                                "array index must contain ASCII decimal digits".into(),
                            ));
                        }
                        name.text.parse::<usize>().ok()
                    }
                    (_, Some(Literal::Int(index))) if *index >= 0 => usize::try_from(*index).ok(),
                    _ => {
                        return Err(error(
                            segment,
                            "array index must be a nonnegative integer".into(),
                        ))
                    }
                };
                index.and_then(|index| values.get(index)).ok_or_else(|| {
                    error(
                        segment,
                        format!("array index is out of bounds for length {}", values.len()),
                    )
                })?
            }
            _ => return Err(error(segment, "value is neither a map nor an array".into())),
        };
    }
    Ok(value.clone())
}

fn evaluate_block(block: &Block, context: &mut Context) -> CompletionResult {
    for statement in &block.statements {
        match evaluate_statement(statement, context)? {
            Completion::Normal(_) => (),
            control => return Ok(control),
        }
    }
    Ok(Completion::Normal(Literal::None))
}

fn evaluate_for(
    binding: &str,
    iterable: &Expr,
    body: &Block,
    context: &mut Context,
) -> CompletionResult {
    let Literal::Array(values) = evaluate_expression(iterable, context)? else {
        return Err(Diagnostic::new(BWErr::OperationIncompatibleError(
            "For requires an array to iterate over".into(),
        ))
        .at_expression(&iterable.span));
    };
    let previous = context.frames[context.current].variables.remove(binding);
    let result = (|| {
        for value in values {
            context.set_variable(binding.to_owned(), value);
            match evaluate_block(body, context)? {
                Completion::Normal(_) | Completion::Continue => (),
                Completion::Break => break,
                returned @ Completion::Return(_) => return Ok(returned),
            }
        }
        Ok(Completion::Normal(Literal::None))
    })();
    if let Some(value) = previous {
        context.set_variable(binding.to_owned(), value);
    } else {
        context.frames[context.current].variables.remove(binding);
    }
    result
}

fn evaluate_while(condition: &Expr, body: &Block, context: &mut Context) -> CompletionResult {
    loop {
        let Literal::Bool(should_loop) = evaluate_expression(condition, context)? else {
            return Err(Diagnostic::new(BWErr::OperationIncompatibleError(
                "While requires a boolean condition".into(),
            ))
            .at_expression(&condition.span));
        };
        if !should_loop {
            break;
        }
        match evaluate_block(body, context)? {
            Completion::Normal(_) | Completion::Continue => (),
            Completion::Break => break,
            returned @ Completion::Return(_) => return Ok(returned),
        }
    }
    Ok(Completion::Normal(Literal::None))
}

fn evaluate_handler(
    binding: Option<&Name>,
    handler: &Block,
    original: Diagnostic,
    context: &mut Context,
) -> CompletionResult {
    let owner = context.current;
    let previous =
        binding.and_then(|name| context.set_variable(name.text.clone(), original.to_value()));
    context.handlers.push(HandledError {
        invocation: owner,
        diagnostic: original.clone(),
    });
    let result = evaluate_block(handler, context).map_err(|error| error.while_handling(original));
    context.handlers.pop();
    if let Some(name) = binding {
        let variables = &mut context.frames[owner].variables;
        if let Some(value) = previous {
            variables.insert(name.text.clone(), value);
        } else {
            variables.remove(&name.text);
        }
    }
    result
}

fn evaluate_statement(statement: &Statement, context: &mut Context) -> CompletionResult {
    evaluate_statement_inner(statement, context)
        .map_err(|error| error.at(&statement.span).capture_stack(&context.calls))
}

fn evaluate_statement_inner(statement: &Statement, context: &mut Context) -> CompletionResult {
    match &statement.kind {
        StatementKind::Assign { name, value } => {
            let value = match value {
                AssignmentValue::Expression(expression) => {
                    evaluate_expression(expression, context)?
                }
                AssignmentValue::Call(call) => invoke(call, context)?,
            };
            context.set_variable(name.text.clone(), value.clone());
            Ok(Completion::Normal(value))
        }
        StatementKind::Define(definition) => {
            let statements = &mut context.frames[context.current].statements;
            if let Some(original) = statements.get(&definition.signature) {
                let origin_span = match original {
                    StmtType::UserDefined(definition) => Some(&definition.span),
                    StmtType::Native { .. } => None,
                };
                let original = match original {
                    StmtType::Native { name, .. } => format!("<builtin {name}>"),
                    StmtType::UserDefined(original) => original.span.location(),
                };
                let mut diagnostic = Diagnostic::new(BWErr::DuplicateStatement {
                    signature: definition.signature.clone(),
                    original,
                    duplicate: definition.span.location(),
                })
                .at(&definition.span);
                if let Some(span) = origin_span {
                    diagnostic = diagnostic.with_related("first definition", span);
                }
                return Err(diagnostic);
            }
            statements.insert(
                definition.signature.clone(),
                StmtType::UserDefined(Arc::clone(definition)),
            );
            Ok(Completion::Normal(Literal::None))
        }
        StatementKind::Invoke(call) => invoke(call, context).map(Completion::Normal),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let Literal::Bool(condition) = evaluate_expression(condition, context)? else {
                return Err(Diagnostic::new(BWErr::OperationIncompatibleError(
                    "If requires a boolean condition".into(),
                ))
                .at_expression(&condition.span));
            };
            if condition {
                evaluate_block(then_branch, context)
            } else {
                match else_branch {
                    Some(ElseBranch::Block(block)) => evaluate_block(block, context),
                    Some(ElseBranch::If(statement)) => evaluate_statement(statement, context),
                    None => Ok(Completion::Normal(Literal::None)),
                }
            }
        }
        StatementKind::For {
            binding,
            iterable,
            body,
        } => evaluate_for(&binding.text, iterable, body, context),
        StatementKind::While { condition, body } => evaluate_while(condition, body, context),
        StatementKind::Try {
            body,
            binding,
            handler,
        } => match evaluate_block(body, context) {
            Ok(value) => Ok(value),
            Err(original) => evaluate_handler(binding.as_ref(), handler, original, context),
        },
        StatementKind::Return(expression) => {
            let value = match expression {
                Some(expression) => evaluate_expression(expression, context)?,
                None => Literal::None,
            };
            Ok(Completion::Return(value))
        }
        StatementKind::Break => Ok(Completion::Break),
        StatementKind::Continue => Ok(Completion::Continue),
        StatementKind::Rethrow => {
            match context
                .handlers
                .last()
                .filter(|handler| handler.invocation == context.current)
            {
                Some(handler) => Err(handler
                    .diagnostic
                    .clone()
                    .with_related("rethrow", &statement.span)),
                None => Err(BWErr::ControlFlowError(
                    "Rethrow requires an enclosing Catch in the same invocation".into(),
                )
                .into()),
            }
        }
    }
}

// Retain runtime boundary guards even though public entry points validate placement.
fn finish_script(completion: Completion) -> RuntimeResult {
    match completion {
        Completion::Normal(value) => Ok(value),
        Completion::Return(_) => {
            Err(BWErr::ControlFlowError("Return requires a custom-statement body".into()).into())
        }
        Completion::Break => Err(BWErr::ControlFlowError(
            "Break requires an enclosing loop in the same invocation".into(),
        )
        .into()),
        Completion::Continue => Err(BWErr::ControlFlowError(
            "Continue requires an enclosing loop in the same invocation".into(),
        )
        .into()),
    }
}

/// Evaluate an already parsed, owned statement at script level in this context.
///
/// Definitions retain their syntax tree and source spans after the program is dropped.
/// The entire statement is validated before execution; custom calls consume their returns.
pub fn execute_statement(statement: &Statement, context: &mut Context) -> LiteralResult {
    execute_statement_detailed(statement, context).map_err(Diagnostic::into_error)
}

/// Execute one script-level statement with source locations and entered-call frames.
pub fn execute_statement_detailed(statement: &Statement, context: &mut Context) -> RuntimeResult {
    ast::validate_script_detailed(std::slice::from_ref(statement))?;
    finish_script(evaluate_statement(statement, context)?)
}

/// Validate the complete program, then execute without parsing or rebuilding it.
pub fn evaluate_program(program: &Program, context: &mut Context) -> LiteralResult {
    evaluate_program_detailed(program, context).map_err(Diagnostic::into_error)
}

/// Validate and execute a program while preserving structured diagnostic causes.
pub fn evaluate_program_detailed(program: &Program, context: &mut Context) -> RuntimeResult {
    program.validate_detailed()?;
    let mut result = Literal::None;
    for statement in &program.statements {
        result = finish_script(evaluate_statement(statement, context)?)?;
    }
    Ok(result)
}

/// Compatibility entry point for callers that already hold a Pest pair.
///
/// This lowers the pair once. Prefer `Program::parse` and `evaluate_program` to
/// share one owned source allocation across an entire script.
pub fn botwork(pair: Pair<Rule>, context: &mut Context) -> LiteralResult {
    botwork_detailed(pair, context).map_err(Diagnostic::into_error)
}

/// Parser-pair compatibility with detailed execution errors.
pub fn botwork_detailed(pair: Pair<Rule>, context: &mut Context) -> RuntimeResult {
    match ast::from_pair(pair)? {
        Node::Statement(statement) => execute_statement_detailed(&statement, context),
        Node::Expression(expression) => evaluate_expression(&expression, context),
        Node::Block(block) => {
            ast::validate_script_detailed(&block.statements)?;
            finish_script(evaluate_block(&block, context)?)
        }
        Node::None => Ok(Literal::None),
    }
}
