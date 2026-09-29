//! Static checks over parsed scripts and suites. Nothing is executed: no
//! statement runs, no module is loaded, and no host operation is called.
//!
//! Each finding names a stable rule. A finding that predicts a runtime error
//! carries that error's diagnostic code, so `--check` and a failing run agree.
//! Name resolution mirrors the runtime: definitions register when they execute,
//! definition bodies resolve names when they are called, and nested scopes may
//! shadow built-ins while the root scope may not.
use super::{
    ast::{
        normalize_sentence, suite::Suite, AccessSegment, AssignmentValue, Block, Call, ElseBranch,
        Expr, ExprKind, Program, Span, Statement, StatementKind, UnaryOp,
    },
    diagnostic::DiagnosticCode,
    eval::Context,
    run::RunLimits,
    signature::{StatementSignature, ValueKind},
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
};

/// How serious a finding is. Errors predict a failing run; warnings flag code
/// that is likely wrong but may be intended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A check with a stable name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// A call that no built-in, definition, or import can satisfy.
    UndefinedStatement,
    /// A call that runs before the definition it names has registered.
    StatementBeforeDefinition,
    /// A read of a variable that no reachable scope ever assigns.
    UndefinedVariable,
    /// A second definition with the same signature in one scope, or a root
    /// definition that redefines a built-in.
    DuplicateStatement,
    /// A statement after one that always leaves its block.
    UnreachableCode,
    /// A literal argument of a kind the built-in parameter never accepts.
    ArgumentKind,
    /// A literal `If`/`While` condition that is not a boolean, or a literal
    /// `For` input that is not an array.
    ConditionKind,
}

impl Rule {
    pub const ALL: [Self; 7] = [
        Self::UndefinedStatement,
        Self::StatementBeforeDefinition,
        Self::UndefinedVariable,
        Self::DuplicateStatement,
        Self::UnreachableCode,
        Self::ArgumentKind,
        Self::ConditionKind,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UndefinedStatement => "undefined-statement",
            Self::StatementBeforeDefinition => "statement-before-definition",
            Self::UndefinedVariable => "undefined-variable",
            Self::DuplicateStatement => "duplicate-statement",
            Self::UnreachableCode => "unreachable-code",
            Self::ArgumentKind => "argument-kind",
            Self::ConditionKind => "condition-kind",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::UndefinedStatement
            | Self::DuplicateStatement
            | Self::ArgumentKind
            | Self::ConditionKind => Severity::Error,
            Self::StatementBeforeDefinition | Self::UndefinedVariable | Self::UnreachableCode => {
                Severity::Warning
            }
        }
    }

    /// The runtime diagnostic this rule predicts, if any.
    pub fn code(self) -> Option<DiagnosticCode> {
        match self {
            Self::UndefinedStatement | Self::StatementBeforeDefinition => {
                Some(DiagnosticCode::UndefinedStatement)
            }
            Self::UndefinedVariable => Some(DiagnosticCode::UndefinedVariable),
            Self::DuplicateStatement => Some(DiagnosticCode::DuplicateStatement),
            Self::ArgumentKind | Self::ConditionKind => Some(DiagnosticCode::IncompatibleType),
            Self::UnreachableCode => None,
        }
    }
}

/// One problem found without running the program.
#[derive(Clone, Debug)]
pub struct Finding {
    pub rule: Rule,
    pub span: Span,
    pub message: String,
    pub help: String,
}

impl Finding {
    pub fn severity(&self) -> Severity {
        self.rule.severity()
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (line, column) = self.span.line_column();
        let (end_line, end_column) = self.span.end_line_column();
        write!(
            output,
            "{}:{line}:{column}-{end_line}:{end_column}: {}[{}]: ",
            self.span.source().name(),
            self.severity().as_str(),
            self.rule.as_str()
        )?;
        if let Some(code) = self.rule.code() {
            write!(output, "[{code}] ")?;
        }
        write!(output, "{}\n  help: {}", self.message, self.help)
    }
}

/// Checks programs against a statement catalogue.
pub struct Analyzer {
    builtins: HashMap<String, StatementSignature>,
}

impl Default for Analyzer {
    /// The CLI's built-in catalogue.
    fn default() -> Self {
        let mut context =
            Context::with_limits(RunLimits::default()).expect("default limits are valid");
        context.init_statements();
        Self::with_statements(context.statement_signatures())
    }
}

impl Analyzer {
    /// Check against these host statements, such as an `Engine`'s registrations.
    pub fn with_statements<'a>(
        signatures: impl IntoIterator<Item = &'a StatementSignature>,
    ) -> Self {
        Self {
            builtins: signatures
                .into_iter()
                .map(|signature| (signature.normalized().to_owned(), signature.clone()))
                .collect(),
        }
    }

    /// Findings for a script, in source order.
    pub fn check_program(&self, program: &Program) -> Vec<Finding> {
        self.check(&[(program, &HashSet::new())])
    }

    /// Findings for a suite's setup, teardown, and every case, in source order.
    /// Cases see the variables their suite setup assigns and their row binding.
    pub fn check_suite(&self, suite: &Suite) -> Vec<Finding> {
        let fixtures = suite.fixture_programs();
        let mut shared = HashSet::new();
        collect_frame(
            &fixtures.setup.statements,
            &mut FrameNames::default(),
            &mut shared,
        );
        let shared: HashSet<String> = shared.into_iter().map(str::to_owned).collect();
        let mut programs = vec![
            (fixtures.setup, HashSet::new()),
            (fixtures.teardown, shared.clone()),
        ];
        for (index, case) in suite.cases().iter().enumerate() {
            let mut inputs = shared.clone();
            if let Some(binding) = case.binding() {
                inputs.insert(binding.to_owned());
            }
            programs.push((suite.program(index).expect("case index"), inputs));
        }
        let programs: Vec<_> = programs
            .iter()
            .map(|(program, inputs)| (program, inputs))
            .collect();
        self.check(&programs)
    }

    fn check(&self, programs: &[(&Program, &HashSet<String>)]) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (program, inputs) in programs {
            let mut checker = Checker {
                builtins: &self.builtins,
                findings: &mut findings,
                reported: HashSet::new(),
            };
            let frame = Frame::new(None, &program.statements, &[], inputs);
            checker.duplicates(&frame, true);
            checker.block(&program.statements, &frame);
        }
        // Library and fixture statements appear in every case program.
        let mut seen = HashSet::new();
        findings.retain(|finding| {
            seen.insert((
                finding.rule,
                finding.span.source().name().to_owned(),
                finding.span.start(),
                finding.message.clone(),
            ))
        });
        findings.sort_by_key(|finding| (finding.span.start(), finding.rule));
        findings
    }
}

#[derive(Default)]
struct FrameNames<'a> {
    definitions: HashMap<&'a str, Vec<&'a Statement>>,
    namespaces: HashSet<String>,
}

/// Collect what one invocation frame binds. Control blocks share their frame;
/// definition bodies are frames of their own.
fn collect_frame<'a>(
    statements: &'a [Statement],
    names: &mut FrameNames<'a>,
    variables: &mut HashSet<&'a str>,
) {
    for statement in statements {
        match statement.kind() {
            StatementKind::Assign { name, .. } => {
                variables.insert(&name.text);
            }
            StatementKind::Define(definition) => names
                .definitions
                .entry(&definition.signature)
                .or_default()
                .push(statement),
            StatementKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_frame(&then_branch.statements, names, variables);
                match else_branch {
                    Some(ElseBranch::Block(block)) => {
                        collect_frame(&block.statements, names, variables)
                    }
                    Some(ElseBranch::If(nested)) => {
                        collect_frame(std::slice::from_ref(nested.as_ref()), names, variables)
                    }
                    None => {}
                }
            }
            StatementKind::For { binding, body, .. } => {
                variables.insert(&binding.text);
                collect_frame(&body.statements, names, variables);
            }
            StatementKind::While { body, .. } | StatementKind::Poll { body, .. } => {
                collect_frame(&body.statements, names, variables)
            }
            StatementKind::Try {
                body,
                binding,
                handler,
            } => {
                if let Some(binding) = binding {
                    variables.insert(&binding.text);
                }
                collect_frame(&body.statements, names, variables);
                collect_frame(&handler.statements, names, variables);
            }
            StatementKind::Finally { body, cleanup } => {
                collect_frame(&body.statements, names, variables);
                collect_frame(&cleanup.statements, names, variables);
            }
            StatementKind::Import { namespace, .. } => {
                names.namespaces.insert(normalize_sentence(&namespace.text));
            }
            StatementKind::Invoke(_)
            | StatementKind::Return(_)
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Rethrow => {}
        }
    }
}

struct Frame<'a> {
    parent: Option<&'a Frame<'a>>,
    names: FrameNames<'a>,
    variables: HashSet<&'a str>,
    inputs: &'a HashSet<String>,
}

impl<'a> Frame<'a> {
    fn new(
        parent: Option<&'a Frame<'a>>,
        statements: &'a [Statement],
        parameters: &'a [super::ast::Name],
        inputs: &'a HashSet<String>,
    ) -> Self {
        let mut names = FrameNames::default();
        let mut variables: HashSet<&str> =
            parameters.iter().map(|name| name.text.as_str()).collect();
        collect_frame(statements, &mut names, &mut variables);
        Self {
            parent,
            names,
            variables,
            inputs,
        }
    }

    fn chain(&self) -> impl Iterator<Item = &Frame<'a>> {
        std::iter::successors(Some(self), |frame| frame.parent)
    }

    fn binds(&self, variable: &str) -> bool {
        self.chain()
            .any(|frame| frame.variables.contains(variable) || frame.inputs.contains(variable))
    }

    fn imports(&self, namespace: &str) -> bool {
        self.chain()
            .any(|frame| frame.names.namespaces.contains(namespace))
    }

    /// The user definition a call reaches: its own frame first, then enclosing ones.
    fn definition(&self, signature: &str) -> Option<(bool, &[&'a Statement])> {
        self.chain().enumerate().find_map(|(depth, frame)| {
            frame
                .names
                .definitions
                .get(signature)
                .map(|definitions| (depth == 0, definitions.as_slice()))
        })
    }
}

struct Checker<'a, 'f> {
    builtins: &'a HashMap<String, StatementSignature>,
    findings: &'f mut Vec<Finding>,
    /// Undefined variables are reported at their first read only.
    reported: HashSet<String>,
}

impl Checker<'_, '_> {
    fn push(&mut self, rule: Rule, span: &Span, message: String, help: impl Into<String>) {
        self.findings.push(Finding {
            rule,
            span: span.clone(),
            message,
            help: help.into(),
        });
    }

    fn duplicates(&mut self, frame: &Frame<'_>, root: bool) {
        let mut signatures: Vec<_> = frame.names.definitions.iter().collect();
        signatures.sort_by_key(|(_, statements)| statements[0].span.start());
        for (signature, statements) in signatures {
            if root {
                if let Some(builtin) = self.builtins.get(*signature) {
                    self.push(
                        Rule::DuplicateStatement,
                        &statements[0].span,
                        format!(
                            "This definition redefines the built-in `{}`",
                            builtin.header().text().trim()
                        ),
                        "Rename it; only definitions inside another definition may shadow a built-in.",
                    );
                }
            }
            for duplicate in &statements[1..] {
                self.push(
                    Rule::DuplicateStatement,
                    &duplicate.span,
                    format!(
                        "Duplicate definition of `{signature}`; first defined at {}",
                        statements[0].span.location()
                    ),
                    "Rename this definition or remove it; the first one stays registered.",
                );
            }
        }
    }

    fn block(&mut self, statements: &[Statement], frame: &Frame<'_>) {
        // Only the first statement after the first exit is reported.
        let mut exit: Option<&Statement> = None;
        let mut reported = false;
        for statement in statements {
            if let Some(exit) = exit.filter(|_| !reported) {
                reported = true;
                self.push(
                    Rule::UnreachableCode,
                    &statement.span,
                    "This statement never runs".into(),
                    format!(
                        "The statement at {} always leaves the block; remove this code or move it earlier.",
                        exit.span.location()
                    ),
                );
            }
            self.statement(statement, frame);
            if exit.is_none() && self.leaves(statement, frame) {
                exit = Some(statement);
            }
        }
    }

    /// Whether a statement always leaves its enclosing block.
    fn leaves(&self, statement: &Statement, frame: &Frame<'_>) -> bool {
        let block_leaves = |block: &Block| {
            block
                .statements
                .iter()
                .any(|statement| self.leaves(statement, frame))
        };
        match statement.kind() {
            StatementKind::Return(_)
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Rethrow => true,
            StatementKind::Invoke(call) => {
                call.signature == "fail|param|"
                    && frame.definition(&call.signature).is_none()
                    && self.builtins.contains_key(&call.signature)
            }
            StatementKind::If {
                then_branch,
                else_branch: Some(else_branch),
                ..
            } => {
                block_leaves(then_branch)
                    && match else_branch {
                        ElseBranch::Block(block) => block_leaves(block),
                        ElseBranch::If(nested) => self.leaves(nested, frame),
                    }
            }
            // An error in the body reaches the handler, so both must leave.
            StatementKind::Try { body, handler, .. } => block_leaves(body) && block_leaves(handler),
            StatementKind::Finally { body, cleanup } => block_leaves(body) || block_leaves(cleanup),
            _ => false,
        }
    }

    fn statement(&mut self, statement: &Statement, frame: &Frame<'_>) {
        match statement.kind() {
            StatementKind::Assign { value, .. } => match value {
                AssignmentValue::Expression(expression) => self.expression(expression, frame),
                AssignmentValue::Call(call) => self.call(call, frame),
            },
            StatementKind::Define(definition) => {
                let child = Frame::new(
                    Some(frame),
                    &definition.body.statements,
                    &definition.parameters,
                    frame.inputs,
                );
                self.duplicates(&child, false);
                self.block(&definition.body.statements, &child);
            }
            StatementKind::Invoke(call) => self.call(call, frame),
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.condition(condition, "If requires a boolean condition");
                self.expression(condition, frame);
                self.block(&then_branch.statements, frame);
                match else_branch {
                    Some(ElseBranch::Block(block)) => self.block(&block.statements, frame),
                    Some(ElseBranch::If(nested)) => self.statement(nested, frame),
                    None => {}
                }
            }
            StatementKind::For { iterable, body, .. } => {
                if literal_kind(iterable).is_some_and(|kind| kind != ValueKind::Array) {
                    self.push(
                        Rule::ConditionKind,
                        &iterable.span,
                        "For requires an array to iterate over".into(),
                        "Iterate over an array literal or a variable that holds an array.",
                    );
                }
                self.expression(iterable, frame);
                self.block(&body.statements, frame);
            }
            StatementKind::While { condition, body } => {
                self.condition(condition, "While requires a boolean condition");
                self.expression(condition, frame);
                self.block(&body.statements, frame);
            }
            StatementKind::Poll { options, body, .. } => {
                self.expression(options, frame);
                self.block(&body.statements, frame);
            }
            StatementKind::Try { body, handler, .. } => {
                self.block(&body.statements, frame);
                self.block(&handler.statements, frame);
            }
            StatementKind::Finally { body, cleanup } => {
                self.block(&body.statements, frame);
                self.block(&cleanup.statements, frame);
            }
            StatementKind::Return(Some(expression)) => self.expression(expression, frame),
            StatementKind::Return(None)
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Rethrow
            | StatementKind::Import { .. } => {}
        }
    }

    fn condition(&mut self, condition: &Expr, message: &str) {
        if literal_kind(condition).is_some_and(|kind| kind != ValueKind::Bool) {
            self.push(
                Rule::ConditionKind,
                &condition.span,
                message.into(),
                "Conditions have no truthiness; compare the value to produce a boolean.",
            );
        }
    }

    fn expression(&mut self, expression: &Expr, frame: &Frame<'_>) {
        match &expression.kind {
            ExprKind::Variable(name) => {
                if !frame.binds(name) && self.reported.insert(name.clone()) {
                    self.push(
                        Rule::UndefinedVariable,
                        &expression.span,
                        format!("`{name}` is never assigned in a scope that reaches this read"),
                        "Assign it first, or supply it as an input variable with --var or --vars-file.",
                    );
                }
            }
            ExprKind::Call(call) => self.call(call, frame),
            ExprKind::Access { base, segments } => {
                self.expression(base, frame);
                for segment in segments {
                    if let AccessSegment::Computed { index, .. } = segment {
                        self.expression(index, frame);
                    }
                }
            }
            ExprKind::Array(values) => values
                .iter()
                .for_each(|value| self.expression(value, frame)),
            ExprKind::Map(entries) => entries
                .iter()
                .for_each(|(_, value)| self.expression(value, frame)),
            ExprKind::Unary { operand, .. } => self.expression(operand, frame),
            ExprKind::Binary { left, right, .. } => {
                self.expression(left, frame);
                self.expression(right, frame);
            }
            ExprKind::Integer(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::String(_) => {
            }
        }
    }

    fn call(&mut self, call: &Call, frame: &Frame<'_>) {
        call.arguments
            .iter()
            .for_each(|argument| self.expression(argument, frame));
        let signature = call.signature.as_str();
        if let Some((namespace, _)) = signature.split_once("::") {
            if !frame.imports(namespace) {
                self.push(
                    Rule::UndefinedStatement,
                    &call.span,
                    format!("No module is imported as `{namespace}`"),
                    format!("Import a module with `Import |\"file.botwork\"| As |{namespace}|` before calling it."),
                );
            }
            // Imported statements are checked with their module.
            return;
        }
        // Resolution order at the moment of the call: a definition this frame
        // registered earlier, then enclosing frames, then the built-ins.
        let own = frame.names.definitions.get(signature);
        if own.is_some_and(|definitions| {
            definitions
                .iter()
                .any(|definition| definition.span.start() < call.span.start())
        }) || frame
            .chain()
            .skip(1)
            .any(|outer| outer.names.definitions.contains_key(signature))
        {
            return;
        }
        if let Some(builtin) = self.builtins.get(signature) {
            self.arguments(call, builtin);
            return;
        }
        if let Some(definitions) = own {
            self.push(
                Rule::StatementBeforeDefinition,
                &call.span,
                format!(
                    "`{}` is called before its definition at {} registers",
                    call.span.text().trim(),
                    definitions[0].span.location()
                ),
                "Definitions register when they run; move the definition above this call.",
            );
            return;
        }
        let help = match self.suggestion(signature, frame) {
            Some(header) => format!(
                "Did you mean `{header}`? Calls must match a definition's words and parameter positions."
            ),
            None => {
                "Define the statement before calling it, or check its words and parameter positions."
                    .into()
            }
        };
        self.push(
            Rule::UndefinedStatement,
            &call.span,
            format!("Statement not defined: {}", call.span.text().trim()),
            help,
        );
    }

    /// Literal arguments must be kinds the built-in's parameters accept.
    fn arguments(&mut self, call: &Call, builtin: &StatementSignature) {
        for (index, (argument, parameter)) in
            call.arguments.iter().zip(builtin.parameters()).enumerate()
        {
            let Some(kind) = literal_kind(argument) else {
                continue;
            };
            if !parameter.accepted.contains(kind) {
                self.push(
                    Rule::ArgumentKind,
                    &argument.span,
                    format!(
                        "Parameter `{}` (argument {}) of `{}` requires {}; got {}",
                        parameter.name,
                        index + 1,
                        call.signature,
                        parameter.accepted,
                        kind.as_str()
                    ),
                    format!(
                        "Pass a value that `{}` accepts; see --statement-help.",
                        builtin.header().text().trim()
                    ),
                );
            }
        }
    }

    /// A known statement with the same words but other parameter positions.
    fn suggestion(&self, signature: &str, frame: &Frame<'_>) -> Option<String> {
        let words = |signature: &str| {
            signature
                .replace("|param|", " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let wanted = words(signature);
        let defined =
            frame.chain().flat_map(|frame| {
                frame.names.definitions.iter().filter_map(
                    |(candidate, statements)| match statements[0].kind() {
                        StatementKind::Define(definition) => {
                            Some((*candidate, definition.header.text().trim().to_owned()))
                        }
                        _ => None,
                    },
                )
            });
        let mut candidates: BTreeSet<(String, String)> = defined
            .chain(self.builtins.iter().map(|(candidate, builtin)| {
                (
                    candidate.as_str(),
                    builtin.header().text().trim().to_owned(),
                )
            }))
            .filter(|(candidate, _)| words(candidate) == wanted)
            .map(|(candidate, header)| (candidate.to_owned(), header))
            .collect();
        candidates.pop_first().map(|(_, header)| header)
    }
}

/// The kind of a literal expression, including a negated number.
fn literal_kind(expression: &Expr) -> Option<ValueKind> {
    match &expression.kind {
        ExprKind::Integer(_) => Some(ValueKind::Int),
        ExprKind::Float(_) => Some(ValueKind::Float),
        ExprKind::Bool(_) => Some(ValueKind::Bool),
        ExprKind::String(_) => Some(ValueKind::String),
        ExprKind::Array(_) => Some(ValueKind::Array),
        ExprKind::Map(_) => Some(ValueKind::Map),
        ExprKind::Unary {
            operator: UnaryOp::Negate,
            operand,
            ..
        } => literal_kind(operand).filter(|kind| matches!(kind, ValueKind::Int | ValueKind::Float)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
