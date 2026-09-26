use pest::iterators::Pair;
use std::{
    collections::HashMap,
    io::{self, Write},
    sync::Arc,
};

use super::{
    ast::{
        self, AssignmentValue, BinaryOp, Block, Call, Definition, ElseBranch, Expr, ExprKind, Node,
        Program, Statement, StatementKind,
    },
    grammar::{finite_float, BWErr, Literal, LiteralResult, Operate, Rule},
};

#[cfg(test)]
mod tests;

#[derive(Debug)]
enum Completion {
    Normal(Literal),
    Return(Literal),
    Break,
    Continue,
}

type CompletionResult = Result<Completion, BWErr>;

#[derive(Clone)]
enum StmtType {
    Native(Callback),
    UserDefined(Arc<Definition>),
}

#[derive(Clone, Default)]
pub struct Context {
    variables: HashMap<String, Literal>,
    statements: HashMap<String, StmtType>,
    #[cfg(test)]
    expression_visits: Vec<String>,
}

impl Context {
    fn get_variable(&self, name: &str) -> LiteralResult {
        self.variables
            .get(name)
            .cloned()
            .ok_or_else(|| BWErr::VariableNotDefined(name.to_owned()))
    }

    fn set_variable(&mut self, name: String, literal: Literal) -> Option<Literal> {
        self.variables.insert(name, literal)
    }

    pub fn init_statements(&mut self) {
        self.statements
            .insert("log|param|".into(), StmtType::Native(log_param));
    }
}

type Callback = fn(&Call, &mut Context) -> LiteralResult;

fn log_param(call: &Call, context: &mut Context) -> LiteralResult {
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

fn invoke(call: &Call, context: &mut Context) -> LiteralResult {
    let definition = context
        .statements
        .get(&call.signature)
        .cloned()
        .ok_or_else(|| BWErr::StatementNotDefined(call.span.text().to_owned()))?;
    match definition {
        StmtType::Native(callback) => callback(call, context),
        StmtType::UserDefined(definition) => {
            if definition.parameters.len() != call.arguments.len() {
                return Err(BWErr::ParameterMissingError(
                    "The call does not match the definition's parameter count".into(),
                ));
            }
            // Argument binding and invocation frames have their own remaining TODO.
            for (parameter, argument) in definition.parameters.iter().zip(&call.arguments) {
                let value = evaluate_expression(argument, context)?;
                context.set_variable(parameter.text.clone(), value);
            }
            match evaluate_block(&definition.body, context)? {
                Completion::Return(value) => Ok(value),
                // Reject unconsumed loop controls here so they cannot reach a caller's loop.
                completion => finish_script(completion),
            }
        }
    }
}

fn evaluate_expression(expression: &Expr, context: &mut Context) -> LiteralResult {
    #[cfg(test)]
    context
        .expression_visits
        .push(expression.span.text().to_owned());

    match &expression.kind {
        ExprKind::Integer(text) => text
            .parse::<i32>()
            .map(Literal::Int)
            .map_err(|error| BWErr::ParsingIntegerError(error.to_string())),
        ExprKind::Float(text) => {
            let value = text
                .parse::<f32>()
                .map_err(|error| BWErr::ParsingIntegerError(error.to_string()))?;
            finite_float(value, "Float literal")
        }
        ExprKind::Bool(value) => Ok(Literal::Bool(*value)),
        ExprKind::String(value) => Ok(Literal::String(value.clone())),
        ExprKind::Variable(name) => context.get_variable(name),
        ExprKind::Access(path) => Err(BWErr::UnsupportedAccessError(path.clone())),
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
        } => operator
            .to_rule()
            .operate_unary(evaluate_expression(operand, context)?),
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
                    )));
                };
                if (*operator == BinaryOp::And && !value) || (*operator == BinaryOp::Or && *value) {
                    return Ok(Literal::Bool(*value));
                }
            }
            let right = evaluate_expression(right, context)?;
            operator.to_rule().operate_binary(left, right)
        }
    }
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
        return Err(BWErr::OperationIncompatibleError(
            "For requires an array to iterate over".into(),
        ));
    };
    for value in values {
        context.set_variable(binding.to_owned(), value);
        match evaluate_block(body, context)? {
            Completion::Normal(_) | Completion::Continue => (),
            Completion::Break => break,
            returned @ Completion::Return(_) => return Ok(returned),
        }
    }
    Ok(Completion::Normal(Literal::None))
}

fn evaluate_while(condition: &Expr, body: &Block, context: &mut Context) -> CompletionResult {
    loop {
        let Literal::Bool(should_loop) = evaluate_expression(condition, context)? else {
            return Err(BWErr::OperationIncompatibleError(
                "While requires a boolean condition".into(),
            ));
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

fn evaluate_statement(statement: &Statement, context: &mut Context) -> CompletionResult {
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
            context.statements.insert(
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
                return Err(BWErr::OperationIncompatibleError(
                    "If requires a boolean condition".into(),
                ));
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
        StatementKind::Try { body, handler } => match evaluate_block(body, context) {
            Ok(value) => Ok(value),
            Err(_) => evaluate_block(handler, context),
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
    }
}

// Runtime guards for public entry points and invocation boundaries. Whole-program
// placement validation, including unused definitions, has its own TODO.
fn finish_script(completion: Completion) -> LiteralResult {
    match completion {
        Completion::Normal(value) => Ok(value),
        Completion::Return(_) => Err(BWErr::ControlFlowError(
            "Return requires a custom-statement body".into(),
        )),
        Completion::Break => Err(BWErr::ControlFlowError(
            "Break requires an enclosing loop in the same invocation".into(),
        )),
        Completion::Continue => Err(BWErr::ControlFlowError(
            "Continue requires an enclosing loop in the same invocation".into(),
        )),
    }
}

/// Evaluate an already parsed, owned statement at script level in this context.
///
/// Definitions retain their syntax tree and source spans after the program is dropped.
/// Escaping control flow is an error; custom calls consume their own returns.
pub fn execute_statement(statement: &Statement, context: &mut Context) -> LiteralResult {
    finish_script(evaluate_statement(statement, context)?)
}

/// Evaluate a program without parsing or rebuilding its statements.
pub fn evaluate_program(program: &Program, context: &mut Context) -> LiteralResult {
    let mut result = Literal::None;
    for statement in &program.statements {
        result = execute_statement(statement, context)?;
    }
    Ok(result)
}

/// Compatibility entry point for callers that already hold a Pest pair.
///
/// This lowers the pair once. Prefer `Program::parse` and `evaluate_program` to
/// share one owned source allocation across an entire script.
pub fn botwork(pair: Pair<Rule>, context: &mut Context) -> LiteralResult {
    match ast::from_pair(pair)? {
        Node::Statement(statement) => execute_statement(&statement, context),
        Node::Expression(expression) => evaluate_expression(&expression, context),
        Node::Block(block) => finish_script(evaluate_block(&block, context)?),
        Node::None => Ok(Literal::None),
    }
}
