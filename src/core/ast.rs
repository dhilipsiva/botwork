//! Owned syntax and original source locations, independent of Pest lifetimes.

use std::{collections::HashMap, sync::Arc};

use pest::{iterators::Pair, Parser};

use super::ast_limits::{self, AstLimits};
use super::diagnostic::{
    CallFrame, Diagnostic, DiagnosticLimits, DiagnosticResult, FormattedDetail,
};
use super::grammar::{BWErr, BWParser, Rule, PRATT_PARSER};
use super::syntax_limits::{SyntaxLimits, DEFAULT_SOURCE_BYTES};

mod parse_diagnostic;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    name: String,
    text: String,
}

impl SourceFile {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    source: Arc<SourceFile>,
    start: usize,
    end: usize,
}

pub(crate) struct LocationDisplay<'a>(&'a Span);

impl std::fmt::Display for LocationDisplay<'_> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str(self.0.source.name())?;
        if output.alternate() {
            // Emergency evidence must never scan source text for coordinates.
            write!(output, ":[byte {}; coordinates omitted]", self.0.start)
        } else {
            let (line, column) = self.0.line_column();
            write!(output, ":{line}:{column}")
        }
    }
}

impl Span {
    /// An input origin without retaining its payload; location refers to input start.
    pub(crate) fn input_origin(name: &str) -> Self {
        Self::source_prefix(name, "", 0)
    }

    pub(crate) fn source_prefix(name: &str, prefix: &str, start: usize) -> Self {
        Self {
            source: Arc::new(SourceFile {
                name: name.into(),
                text: prefix.into(),
            }),
            start,
            end: prefix.len(),
        }
    }
    pub fn source(&self) -> &Arc<SourceFile> {
        &self.source
    }

    pub fn start(&self) -> usize {
        self.start
    }

    pub fn end(&self) -> usize {
        self.end
    }

    pub fn text(&self) -> &str {
        &self.source.text[self.start..self.end]
    }

    /// One-based line and Unicode scalar column; a tab occupies one column.
    /// CRLF is one line ending, and offsets remain UTF-8 byte offsets.
    pub fn line_column(&self) -> (usize, usize) {
        self.position(self.start)
    }

    /// Exclusive end in the same one-based Unicode scalar units as the start.
    pub fn end_line_column(&self) -> (usize, usize) {
        self.position(self.end)
    }

    fn position(&self, offset: usize) -> (usize, usize) {
        let before = &self.source.text[..offset];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count()
            + 1;
        (line, column)
    }

    pub fn location(&self) -> String {
        let (line, column) = self.line_column();
        format!("{}:{line}:{column}", self.source.name())
    }

    pub(crate) fn location_display(&self) -> LocationDisplay<'_> {
        LocationDisplay(self)
    }

    fn of(pair: &Pair<Rule>, source: &Arc<SourceFile>) -> Self {
        let span = pair.as_span();
        Self {
            source: Arc::clone(source),
            start: span.start(),
            end: span.end(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Program {
    pub source: Arc<SourceFile>,
    pub statements: Vec<Statement>,
}

impl Program {
    pub fn parse(name: &str, source: &str) -> Result<Self, BWErr> {
        Self::parse_detailed(name, source).map_err(Diagnostic::into_error)
    }

    /// Parse and validate with retained source locations and structured details.
    pub fn parse_detailed(name: &str, source: &str) -> DiagnosticResult<Self> {
        Self::parse_bounded(name, source, DEFAULT_SOURCE_BYTES, &SyntaxLimits::default())
    }

    /// Parse with a local source-byte budget and tightened syntax guard.
    pub fn parse_bounded(
        name: &str,
        source: &str,
        source_bytes: usize,
        limits: &SyntaxLimits,
    ) -> DiagnosticResult<Self> {
        Self::parse_with_budgets(name, source, source_bytes, limits, &AstLimits::default())
    }

    /// Parse with independent source/syntax and owned-tree admission budgets.
    pub fn parse_with_budgets(
        name: &str,
        source: &str,
        source_bytes: usize,
        limits: &SyntaxLimits,
        ast_limits: &AstLimits,
    ) -> DiagnosticResult<Self> {
        Self::parse_with_reporter(name, source, source_bytes, limits, ast_limits, |failure| {
            failure.default_diagnostic()
        })
    }

    pub(crate) fn parse_with_reporter(
        name: &str,
        source: &str,
        source_bytes: usize,
        limits: &SyntaxLimits,
        ast_limits: &AstLimits,
        report: impl Fn(AstFailure<'_>) -> Diagnostic,
    ) -> DiagnosticResult<Self> {
        ast_limits.validate()?;
        check_source_with_reporter(name, source, source_bytes, limits, &report)?;
        let source = Arc::new(SourceFile {
            name: name.to_owned(),
            text: source.to_owned(),
        });
        let statements = super::parser::BWParser::parse(Rule::botwork, &source.text)
            .map_err(|error| parse_error(error, &source, &report))?
            .filter(|pair| pair.as_rule() != Rule::EOI)
            .map(|pair| {
                let span = Span::of(&pair, &source);
                statement(pair, &source).map_err(|error| Diagnostic::new(error).at(&span))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let program = Self { source, statements };
        program.validate_with_reporter(ast_limits, source_bytes, report)?;
        Ok(program)
    }

    /// Check control placement and parameter names, including unreachable bodies.
    /// This does not evaluate expressions or resolve names.
    pub fn validate(&self) -> Result<(), BWErr> {
        self.validate_detailed().map_err(Diagnostic::into_error)
    }

    pub fn validate_detailed(&self) -> DiagnosticResult<()> {
        self.validate_with_limits(&AstLimits::default(), DEFAULT_SOURCE_BYTES)
    }

    /// Admit all reachable nodes and source owners before control validation.
    pub fn validate_with_limits(
        &self,
        limits: &AstLimits,
        source_bytes: usize,
    ) -> DiagnosticResult<()> {
        self.validate_with_reporter(limits, source_bytes, |failure| failure.default_diagnostic())
    }

    pub(crate) fn validate_with_reporter(
        &self,
        limits: &AstLimits,
        source_bytes: usize,
        report: impl Fn(AstFailure<'_>) -> Diagnostic,
    ) -> DiagnosticResult<()> {
        ast_limits::check_program(self, limits, source_bytes)?;
        validate_control_script(&self.statements).map_err(report)
    }
}

pub(crate) fn check_source(
    name: &str,
    source: &str,
    source_bytes: usize,
    limits: &SyntaxLimits,
) -> DiagnosticResult<()> {
    check_source_with_reporter(name, source, source_bytes, limits, |failure| {
        failure.default_diagnostic()
    })
}

pub(crate) fn check_source_with_reporter(
    name: &str,
    source: &str,
    source_bytes: usize,
    limits: &SyntaxLimits,
    report: impl Fn(AstFailure<'_>) -> Diagnostic,
) -> DiagnosticResult<()> {
    super::syntax_limits::check(source, source_bytes, limits, false).map_err(|violation| {
        let end = violation.offset
            + source[violation.offset..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        report(AstFailure::SourceGuard {
            error: &violation.error,
            name,
            prefix: &source[..end],
            offset: violation.offset,
        })
    })
}

#[derive(Clone, Debug)]
pub struct Statement {
    pub span: Span,
    pub(crate) kind: StatementKind,
}

impl Statement {
    pub fn kind(&self) -> &StatementKind {
        &self.kind
    }

    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            StatementKind::Assign { .. } => "assignment",
            StatementKind::Define(_) => "definition",
            StatementKind::Invoke(_) => "call",
            StatementKind::If { .. } => "if",
            StatementKind::For { .. } => "for",
            StatementKind::While { .. } => "while",
            StatementKind::Try { .. } => "try",
            StatementKind::Return(_) => "return",
            StatementKind::Break => "break",
            StatementKind::Continue => "continue",
            StatementKind::Rethrow => "rethrow",
            StatementKind::Import { .. } => "import",
        }
    }
}

#[derive(Clone, Debug)]
pub enum StatementKind {
    Assign {
        name: Name,
        value: AssignmentValue,
    },
    Define(Arc<Definition>),
    Invoke(Call),
    If {
        condition: Expr,
        then_branch: Block,
        else_branch: Option<ElseBranch>,
    },
    For {
        binding: Name,
        iterable: Expr,
        body: Block,
    },
    While {
        condition: Expr,
        body: Block,
    },
    Try {
        body: Block,
        binding: Option<Name>,
        handler: Block,
    },
    Return(Option<Expr>),
    Break,
    Continue,
    Rethrow,
    Import {
        path: String,
        path_span: Span,
        namespace: Name,
    },
}

#[derive(Clone, Copy, Default)]
struct ControlScope {
    in_definition: bool,
    in_loop: bool,
    in_catch: bool,
}

pub(crate) fn validate_control_script(statements: &[Statement]) -> Result<(), AstFailure<'_>> {
    validate_statements(statements, ControlScope::default())
}

fn validate_statements(
    statements: &[Statement],
    scope: ControlScope,
) -> Result<(), AstFailure<'_>> {
    for statement in statements {
        validate_statement(statement, scope)?;
    }
    Ok(())
}

fn validate_statement(statement: &Statement, scope: ControlScope) -> Result<(), AstFailure<'_>> {
    match &statement.kind {
        StatementKind::Return(_) if !scope.in_definition => Err(control_placement_error(
            statement,
            "Return requires a custom-statement body",
        )),
        StatementKind::Break if !scope.in_loop => Err(control_placement_error(
            statement,
            "Break requires an enclosing loop in the same invocation",
        )),
        StatementKind::Continue if !scope.in_loop => Err(control_placement_error(
            statement,
            "Continue requires an enclosing loop in the same invocation",
        )),
        StatementKind::Rethrow if !scope.in_catch => Err(control_placement_error(
            statement,
            "Rethrow requires an enclosing Catch in the same invocation",
        )),
        StatementKind::Define(definition) => {
            validate_parameters(&definition.parameters)?;
            validate_statements(
                &definition.body.statements,
                ControlScope {
                    in_definition: true,
                    in_loop: false,
                    in_catch: false,
                },
            )
        }
        StatementKind::For { body, .. } | StatementKind::While { body, .. } => validate_statements(
            &body.statements,
            ControlScope {
                in_loop: true,
                ..scope
            },
        ),
        StatementKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            validate_statements(&then_branch.statements, scope)?;
            match else_branch {
                Some(ElseBranch::Block(block)) => validate_statements(&block.statements, scope),
                Some(ElseBranch::If(statement)) => validate_statement(statement, scope),
                None => Ok(()),
            }
        }
        StatementKind::Try { body, handler, .. } => {
            validate_statements(&body.statements, scope)?;
            validate_statements(
                &handler.statements,
                ControlScope {
                    in_catch: true,
                    ..scope
                },
            )
        }
        StatementKind::Assign { .. }
        | StatementKind::Import { .. }
        | StatementKind::Invoke(_)
        | StatementKind::Return(_)
        | StatementKind::Break
        | StatementKind::Continue
        | StatementKind::Rethrow => Ok(()),
    }
}

fn control_placement_error<'a>(statement: &'a Statement, message: &'static str) -> AstFailure<'a> {
    AstFailure::Control {
        span: &statement.span,
        message,
    }
}

/// Borrow parsing/validation evidence until the caller can admit its diagnostic context.
pub(crate) enum AstFailure<'a> {
    SourceGuard {
        error: &'a BWErr,
        name: &'a str,
        prefix: &'a str,
        offset: usize,
    },
    Syntax {
        error: &'a pest::error::Error<Rule>,
        span: &'a Span,
    },
    Control {
        span: &'a Span,
        message: &'static str,
    },
    DuplicateParameter {
        name: &'a str,
        original: &'a Span,
        duplicate: &'a Span,
    },
}

impl AstFailure<'_> {
    pub(crate) fn span(&self) -> Option<&Span> {
        match self {
            Self::Control { span, .. } | Self::Syntax { span, .. } => Some(span),
            Self::DuplicateParameter { duplicate, .. } => Some(duplicate),
            Self::SourceGuard { .. } => None,
        }
    }

    fn default_diagnostic(self) -> Diagnostic {
        self.diagnostic(&DiagnosticLimits::default(), std::iter::empty())
    }

    pub(crate) fn diagnostic<'a>(
        &self,
        limits: &DiagnosticLimits,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Diagnostic {
        match self {
            Self::SourceGuard {
                error,
                name,
                prefix,
                offset,
            } => {
                // Guard errors contain only fixed resource names/numeric limits
                // or a short configuration message; their source is still borrowed.
                limits.source_prefix((*error).clone(), name, prefix, *offset, frames)
            }
            Self::Syntax { error, span } => {
                let detail = parse_diagnostic::ParseDisplay { error, span };
                limits.formatted_evidence(
                    |[detail]| BWErr::ParsingError(detail),
                    [FormattedDetail {
                        full: format_args!("{detail}"),
                        summary: Some(format_args!("{detail:#}")),
                    }],
                    Some(span),
                    false,
                    frames,
                )
            }
            Self::Control { span, message } => {
                let location = span.location_display();
                limits.formatted_evidence(
                    |[detail]| BWErr::ControlFlowError(detail),
                    [FormattedDetail {
                        full: format_args!("{location}: {message}"),
                        summary: Some(format_args!("{location:#}: {message}")),
                    }],
                    Some(span),
                    false,
                    frames,
                )
            }
            Self::DuplicateParameter {
                name,
                original,
                duplicate,
            } => {
                let first = original.location_display();
                let second = duplicate.location_display();
                limits.formatted_related_fields(
                    |[name, original, duplicate]| BWErr::DuplicateParameter {
                        name,
                        original,
                        duplicate,
                    },
                    [
                        FormattedDetail {
                            full: format_args!("{name}"),
                            summary: None,
                        },
                        FormattedDetail {
                            full: format_args!("{first}"),
                            summary: Some(format_args!("{first:#}")),
                        },
                        FormattedDetail {
                            full: format_args!("{second}"),
                            summary: Some(format_args!("{second:#}")),
                        },
                    ],
                    duplicate,
                    ("first parameter", original),
                    frames,
                )
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum AssignmentValue {
    Expression(Expr),
    Call(Call),
}

#[derive(Clone, Debug)]
pub enum ElseBranch {
    Block(Block),
    If(Box<Statement>),
}

#[derive(Clone, Debug)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub span: Span,
    pub statements: Vec<Statement>,
}

#[derive(Clone, Debug)]
pub struct Definition {
    pub span: Span,
    pub header: Span,
    pub signature: String,
    pub parameters: Vec<Name>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub span: Span,
    pub signature: String,
    pub arguments: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub span: Span,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
pub enum AccessSegment {
    Literal(Name),
    Computed { span: Span, index: Expr },
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Integer(String),
    Float(String),
    Bool(bool),
    String(String),
    Variable(String),
    Call(Box<Call>),
    Access {
        base: Box<Expr>,
        segments: Vec<AccessSegment>,
    },
    Array(Vec<Expr>),
    Map(Vec<(Name, Expr)>),
    Unary {
        operator: UnaryOp,
        operator_span: Span,
        operand: Box<Expr>,
    },
    Binary {
        operator: BinaryOp,
        operator_span: Span,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

impl UnaryOp {
    pub fn to_rule(self) -> Rule {
        match self {
            Self::Negate => Rule::minus,
            Self::Not => Rule::logical_not,
        }
    }

    fn from_rule(rule: Rule) -> Result<Self, BWErr> {
        match rule {
            Rule::minus => Ok(Self::Negate),
            Rule::logical_not => Ok(Self::Not),
            _ => Err(invalid("unary operator")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Power,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    And,
    Or,
}

impl BinaryOp {
    pub fn to_rule(self) -> Rule {
        match self {
            Self::Add => Rule::plus,
            Self::Subtract => Rule::minus,
            Self::Multiply => Rule::multiply,
            Self::Divide => Rule::divide,
            Self::Remainder => Rule::modulus,
            Self::Power => Rule::exponent,
            Self::Less => Rule::less_than,
            Self::LessEqual => Rule::less_than_or_equal,
            Self::Greater => Rule::greater_than,
            Self::GreaterEqual => Rule::greater_than_or_equal,
            Self::Equal => Rule::equal,
            Self::NotEqual => Rule::not_equal,
            Self::And => Rule::logical_and,
            Self::Or => Rule::logical_or,
        }
    }

    fn from_rule(rule: Rule) -> Result<Self, BWErr> {
        match rule {
            Rule::plus => Ok(Self::Add),
            Rule::minus => Ok(Self::Subtract),
            Rule::multiply => Ok(Self::Multiply),
            Rule::divide => Ok(Self::Divide),
            Rule::modulus => Ok(Self::Remainder),
            Rule::exponent => Ok(Self::Power),
            Rule::less_than => Ok(Self::Less),
            Rule::less_than_or_equal => Ok(Self::LessEqual),
            Rule::greater_than => Ok(Self::Greater),
            Rule::greater_than_or_equal => Ok(Self::GreaterEqual),
            Rule::equal => Ok(Self::Equal),
            Rule::not_equal => Ok(Self::NotEqual),
            Rule::logical_and => Ok(Self::And),
            Rule::logical_or => Ok(Self::Or),
            _ => Err(invalid("binary operator")),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Statement(Statement),
    Expression(Expr),
    Block(Block),
    None,
}

/// Compatibility entry point for callers still holding a single Pest pair.
/// The resulting node owns the entire original input, including nested offsets.
#[cfg(test)]
pub(crate) fn from_pair(pair: Pair<Rule>) -> DiagnosticResult<Node> {
    from_pair_with_reporter(pair, |failure| failure.default_diagnostic())
}

pub(crate) fn from_pair_with_reporter(
    pair: Pair<Rule>,
    report: impl Fn(AstFailure<'_>) -> Diagnostic,
) -> DiagnosticResult<Node> {
    let source = Arc::new(SourceFile {
        name: "<input>".to_owned(),
        text: pair.as_span().get_input().to_owned(),
    });
    let span = Span::of(&pair, &source);
    match pair.as_rule() {
        Rule::EOI | Rule::logical_not | Rule::seperator => Ok(Node::None),
        Rule::stmt_catch
            if pair
                .clone()
                .into_inner()
                .any(|child| child.as_rule() == Rule::ident) =>
        {
            return Err(report(AstFailure::Control {
                span: &span,
                message: "Catch binding requires an enclosing Try",
            }));
        }
        Rule::stmt_block | Rule::stmt_catch | Rule::stmt_else => {
            block(pair, &source).map(Node::Block)
        }
        Rule::stmt_assign
        | Rule::stmt_define
        | Rule::stmt_invoke
        | Rule::stmt_import
        | Rule::stmt_if
        | Rule::stmt_for
        | Rule::stmt_while
        | Rule::stmt_try
        | Rule::stmt_return
        | Rule::stmt_break
        | Rule::stmt_continue => statement(pair, &source).map(Node::Statement),
        Rule::stmt_rethrow => statement(pair, &source).map(Node::Statement),
        _ => expression(pair, &source).map(Node::Expression),
    }
    .map_err(|error| Diagnostic::new(error).at(&span))
}

fn invalid(part: &'static str) -> BWErr {
    Diagnostic::formatted(
        BWErr::ParsingError,
        format_args!("Invalid {part} in syntax tree"),
    )
    .into_error()
}

fn required<'i>(inner: &mut impl Iterator<Item = Pair<'i, Rule>>) -> Result<Pair<'i, Rule>, BWErr> {
    inner.next().ok_or_else(|| invalid("missing child"))
}

fn finish<'i>(mut inner: impl Iterator<Item = Pair<'i, Rule>>) -> Result<(), BWErr> {
    if inner.next().is_some() {
        Err(invalid("unexpected child"))
    } else {
        Ok(())
    }
}

fn lower_name(pair: Pair<Rule>, source: &Arc<SourceFile>) -> Name {
    Name {
        text: pair.as_str().to_owned(),
        span: Span::of(&pair, source),
    }
}

fn statement(pair: Pair<Rule>, source: &Arc<SourceFile>) -> Result<Statement, BWErr> {
    let span = Span::of(&pair, source);
    let rule = pair.as_rule();
    if rule == Rule::stmt_invoke {
        return Ok(Statement {
            span,
            kind: StatementKind::Invoke(call(pair, source)?),
        });
    }
    let mut inner = pair
        .into_inner()
        .filter(|part| part.as_rule() != Rule::continuation);
    let kind = match rule {
        Rule::stmt_assign => {
            let name = lower_name(required(&mut inner)?, source);
            let value = required(&mut inner)?;
            let value = if value.as_rule() == Rule::stmt_invoke {
                AssignmentValue::Call(call(value, source)?)
            } else {
                AssignmentValue::Expression(expression(value, source)?)
            };
            StatementKind::Assign { name, value }
        }
        Rule::stmt_define => {
            let header = required(&mut inner)?;
            let header_span = Span::of(&header, source);
            let signature = signature(&header)?;
            let parameters = header
                .into_inner()
                .filter(|pair| pair.as_rule() == Rule::ident)
                .map(|pair| lower_name(pair, source))
                .collect();
            let body = block(required(&mut inner)?, source)?;
            StatementKind::Define(Arc::new(Definition {
                span: span.clone(),
                header: header_span,
                signature,
                parameters,
                body,
            }))
        }
        Rule::stmt_if => {
            let condition = expression(required(&mut inner)?, source)?;
            let then_branch = block(required(&mut inner)?, source)?;
            let else_branch = inner
                .next()
                .map(|pair| {
                    let mut children = pair.into_inner();
                    let branch = required(&mut children)?;
                    finish(children)?;
                    match branch.as_rule() {
                        Rule::stmt_block => block(branch, source).map(ElseBranch::Block),
                        Rule::stmt_if => {
                            statement(branch, source).map(Box::new).map(ElseBranch::If)
                        }
                        _ => Err(invalid("else branch")),
                    }
                })
                .transpose()?;
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            }
        }
        Rule::stmt_for => StatementKind::For {
            binding: lower_name(required(&mut inner)?, source),
            iterable: expression(required(&mut inner)?, source)?,
            body: block(required(&mut inner)?, source)?,
        },
        Rule::stmt_while => StatementKind::While {
            condition: expression(required(&mut inner)?, source)?,
            body: block(required(&mut inner)?, source)?,
        },
        Rule::stmt_try => {
            let body = block(required(&mut inner)?, source)?;
            let mut handler = required(&mut inner)?.into_inner();
            let first = required(&mut handler)?;
            let (binding, handler_block) = if first.as_rule() == Rule::ident {
                (
                    Some(lower_name(first, source)),
                    block(required(&mut handler)?, source)?,
                )
            } else {
                (None, block(first, source)?)
            };
            finish(handler)?;
            StatementKind::Try {
                body,
                binding,
                handler: handler_block,
            }
        }
        Rule::stmt_return => StatementKind::Return(
            inner
                .next()
                .map(|pair| expression(pair, source))
                .transpose()?,
        ),
        Rule::stmt_break => StatementKind::Break,
        Rule::stmt_continue => StatementKind::Continue,
        Rule::stmt_rethrow => StatementKind::Rethrow,
        Rule::stmt_import => {
            let path = required(&mut inner)?;
            StatementKind::Import {
                path_span: Span::of(&path, source),
                path: decode_string(path)?,
                namespace: lower_name(required(&mut inner)?, source),
            }
        }
        _ => return Err(invalid("statement")),
    };
    finish(inner)?;
    Ok(Statement { span, kind })
}

fn block(pair: Pair<Rule>, source: &Arc<SourceFile>) -> Result<Block, BWErr> {
    let span = Span::of(&pair, source);
    // Else and unbound Catch wrappers are accepted by the compatibility entry point.
    let mut statements = Vec::new();
    for pair in pair.into_inner() {
        if pair.as_rule() == Rule::stmt_block {
            statements.extend(block(pair, source)?.statements);
        } else {
            statements.push(statement(pair, source)?);
        }
    }
    Ok(Block { span, statements })
}

fn signature(pair: &Pair<Rule>) -> Result<String, BWErr> {
    let mut signature = String::new();
    for part in pair.clone().into_inner() {
        match part.as_rule() {
            Rule::part => {
                signature.push_str(&normalize_sentence(part.as_str()));
            }
            Rule::ident | Rule::param_invoke => signature.push_str("|param|"),
            Rule::continuation => (),
            _ => return Err(invalid("statement signature")),
        }
    }
    Ok(signature)
}

pub(crate) fn normalize_sentence(text: &str) -> String {
    text.chars()
        .filter(|character| !matches!(character, ' ' | '\t'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_error(
    error: pest::error::Error<Rule>,
    source: &Arc<SourceFile>,
    report: impl Fn(AstFailure<'_>) -> Diagnostic,
) -> Diagnostic {
    let (start, end) = match error.location {
        pest::error::InputLocation::Pos(start) => {
            let length = source.text[start..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
            (start, start + length)
        }
        pest::error::InputLocation::Span((start, end)) => (start, end),
    };
    let span = Span {
        source: Arc::clone(source),
        start,
        end,
    };
    report(AstFailure::Syntax {
        error: &error,
        span: &span,
    })
}

fn validate_parameters(parameters: &[Name]) -> Result<(), AstFailure<'_>> {
    let mut seen = HashMap::new();
    for parameter in parameters {
        if let Some(original) = seen.insert(&parameter.text, &parameter.span) {
            return Err(AstFailure::DuplicateParameter {
                name: &parameter.text,
                original,
                duplicate: &parameter.span,
            });
        }
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) struct NativeSignature {
    pub span: Span,
    pub signature: String,
    pub parameters: Vec<Name>,
}

pub(crate) fn native_signature(name: &str, text: &str) -> DiagnosticResult<NativeSignature> {
    check_source(name, text, DEFAULT_SOURCE_BYTES, &SyntaxLimits::default())?;
    let source = Arc::new(SourceFile {
        name: name.into(),
        text: text.into(),
    });
    let mut pairs = BWParser::parse(Rule::native_signature, &source.text)
        .map_err(|error| parse_error(error, &source, |failure| failure.default_diagnostic()))?;
    let header = required(&mut pairs)?;
    let span = Span::of(&header, &source);
    let signature = signature(&header)?;
    if signature.is_empty() {
        return Err(Diagnostic::new(invalid("nonempty native signature")).at(&span));
    }
    let parameters = header
        .into_inner()
        .filter(|pair| pair.as_rule() == Rule::ident)
        .map(|pair| lower_name(pair, &source))
        .collect::<Vec<_>>();
    validate_parameters(&parameters).map_err(AstFailure::default_diagnostic)?;
    Ok(NativeSignature {
        span,
        signature,
        parameters,
    })
}

fn call(pair: Pair<Rule>, source: &Arc<SourceFile>) -> Result<Call, BWErr> {
    let span = Span::of(&pair, source);
    let signature = signature(&pair)?;
    let arguments = pair
        .into_inner()
        .filter(|pair| pair.as_rule() == Rule::param_invoke)
        .map(|pair| expression(pair, source))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Call {
        span,
        signature,
        arguments,
    })
}

fn expression(pair: Pair<Rule>, source: &Arc<SourceFile>) -> Result<Expr, BWErr> {
    let span = Span::of(&pair, source);
    let kind = match pair.as_rule() {
        Rule::integer => ExprKind::Integer(pair.as_str().to_owned()),
        Rule::float => ExprKind::Float(pair.as_str().to_owned()),
        Rule::boolean_true => ExprKind::Bool(true),
        Rule::boolean_false => ExprKind::Bool(false),
        Rule::string => ExprKind::String(decode_string(pair)?),
        Rule::ident => ExprKind::Variable(pair.as_str().to_owned()),
        Rule::primary => {
            let mut inner = pair.into_inner();
            let mut base = expression(required(&mut inner)?, source)?;
            let segments = inner
                .map(|segment| {
                    let span = Span::of(&segment, source);
                    let rule = segment.as_rule();
                    let mut children = segment.into_inner();
                    let child = required(&mut children)?;
                    finish(children)?;
                    match rule {
                        Rule::named_access => Ok(AccessSegment::Literal(lower_name(child, source))),
                        Rule::computed_access => Ok(AccessSegment::Computed {
                            span,
                            index: expression(child, source)?,
                        }),
                        _ => Err(invalid("access segment")),
                    }
                })
                .collect::<Result<Vec<_>, BWErr>>()?;
            if segments.is_empty() {
                base.span = span;
                return Ok(base);
            }
            ExprKind::Access {
                base: Box::new(base),
                segments,
            }
        }
        Rule::keyword => ExprKind::String(pair.as_str().to_owned()),
        Rule::array => ExprKind::Array(
            pair.into_inner()
                .map(|pair| expression(pair, source))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Rule::map => {
            let mut entries = Vec::new();
            for entry in pair.into_inner() {
                let mut children = entry.into_inner();
                let key_pair = required(&mut children)?;
                let mut key = lower_name(key_pair.clone(), source);
                if key_pair.as_rule() == Rule::string {
                    key.text = decode_string(key_pair)?;
                }
                let value = expression(required(&mut children)?, source)?;
                finish(children)?;
                entries.push((key, value));
            }
            ExprKind::Map(entries)
        }
        Rule::unary => {
            let mut inner = pair.into_inner();
            let operator_pair = required(&mut inner)?;
            let operator = UnaryOp::from_rule(operator_pair.as_rule())?;
            let operator_span = Span::of(&operator_pair, source);
            let operand = Box::new(expression(required(&mut inner)?, source)?);
            finish(inner)?;
            ExprKind::Unary {
                operator,
                operator_span,
                operand,
            }
        }
        Rule::call_expression => {
            let mut inner = pair.into_inner();
            let call = call(required(&mut inner)?, source)?;
            finish(inner)?;
            ExprKind::Call(Box::new(call))
        }
        Rule::param_invoke | Rule::braced_expression => {
            let grouped = pair.as_rule() == Rule::braced_expression;
            let mut inner = pair.into_inner();
            let mut value = expression(required(&mut inner)?, source)?;
            finish(inner)?;
            if grouped {
                value.span = span;
            }
            return Ok(value);
        }
        Rule::expression | Rule::power => {
            let mut expr = PRATT_PARSER
                .map_primary(|pair| expression(pair, source))
                .map_infix(|left, operator, right| {
                    let left: Expr = left?;
                    let right: Expr = right?;
                    Ok(Expr {
                        span: Span {
                            source: Arc::clone(source),
                            start: left.span.start,
                            end: right.span.end,
                        },
                        kind: ExprKind::Binary {
                            operator: BinaryOp::from_rule(operator.as_rule())?,
                            operator_span: Span::of(&operator, source),
                            left: Box::new(left),
                            right: Box::new(right),
                        },
                    })
                })
                .parse(pair.into_inner())?;
            expr.span = span;
            return Ok(expr);
        }
        _ => return Err(invalid("expression")),
    };
    Ok(Expr { span, kind })
}

fn decode_string(pair: Pair<Rule>) -> Result<String, BWErr> {
    let content = pair
        .into_inner()
        .next()
        .ok_or_else(|| invalid("string content"))?;
    let mut value = String::new();
    let mut chars = content.as_str().chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            value.push(match chars.next() {
                Some('n') => '\n',
                Some('"') => '"',
                Some('\\') => '\\',
                _ => return Err(invalid("string escape")),
            });
        } else {
            value.push(character);
        }
    }
    Ok(value)
}
