//! Iterative admission checks for owned, possibly host-assembled syntax.

use std::{collections::HashSet, sync::Arc};

use super::{
    ast::*,
    diagnostic::{Diagnostic, DiagnosticResult},
    grammar::BWErr,
};

pub const DEFAULT_AST_NODES: usize = 65_536;
pub const MAX_AST_DEPTH: usize = 128;
pub const DEFAULT_AST_SOURCE_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct AstLimits {
    pub nodes: usize,
    pub depth: usize,
    /// Total bytes across distinct source allocations reachable from one input tree.
    pub source_bytes: usize,
}

impl Default for AstLimits {
    fn default() -> Self {
        Self {
            nodes: DEFAULT_AST_NODES,
            depth: MAX_AST_DEPTH,
            source_bytes: DEFAULT_AST_SOURCE_BYTES,
        }
    }
}

impl AstLimits {
    pub(crate) fn validate(&self) -> DiagnosticResult<()> {
        if self.depth > MAX_AST_DEPTH {
            return Err(BWErr::RunConfiguration(format!(
                "AST depth cannot exceed {MAX_AST_DEPTH}"
            ))
            .into());
        }
        Ok(())
    }
}

enum Item<'a> {
    Statement(&'a Statement),
    Expression(&'a Expr),
    Block(&'a Block),
    Call(&'a Call),
    Definition(&'a Definition),
    Span(&'a Span),
    Segment(&'a AccessSegment),
    Statements(&'a [Statement]),
    Expressions(&'a [Expr]),
    Names(&'a [Name]),
    MapEntries(&'a [(Name, Expr)]),
    Segments(&'a [AccessSegment]),
}

fn limit(resource: &'static str, maximum: usize, span: Option<&Span>) -> Diagnostic {
    let error = Diagnostic::new(BWErr::ResourceLimit {
        resource,
        limit: maximum as u64,
    });
    match span {
        Some(span) => error.at(span),
        None => error,
    }
}

struct Walk<'a> {
    limits: &'a AstLimits,
    per_source_bytes: usize,
    sources: HashSet<*const SourceFile>,
    retained_sources: Option<Vec<Arc<SourceFile>>>,
    source_bytes: usize,
    nodes: usize,
    stack: Vec<(Item<'a>, usize)>,
}

impl<'a> Walk<'a> {
    fn new(limits: &'a AstLimits, per_source_bytes: usize) -> DiagnosticResult<Self> {
        limits.validate()?;
        Ok(Self {
            limits,
            per_source_bytes,
            sources: HashSet::new(),
            retained_sources: None,
            source_bytes: 0,
            nodes: 0,
            stack: vec![],
        })
    }

    fn source(&mut self, source: &Arc<SourceFile>, span: Option<&Span>) -> DiagnosticResult<()> {
        if self.sources.contains(&Arc::as_ptr(source)) {
            return Ok(());
        }
        if source.text().len() > self.per_source_bytes {
            return Err(limit("source bytes", self.per_source_bytes, span));
        }
        self.source_bytes = self
            .source_bytes
            .checked_add(source.text().len())
            .filter(|bytes| *bytes <= self.limits.source_bytes)
            .ok_or_else(|| limit("AST source bytes", self.limits.source_bytes, span))?;
        self.sources.insert(Arc::as_ptr(source));
        if let Some(sources) = &mut self.retained_sources {
            sources.push(Arc::clone(source));
        }
        Ok(())
    }

    fn node(&mut self, span: &Span, depth: usize) -> DiagnosticResult<()> {
        if self.nodes >= self.limits.nodes {
            return Err(limit("AST nodes", self.limits.nodes, Some(span)));
        }
        if depth > self.limits.depth {
            return Err(limit("AST depth", self.limits.depth, Some(span)));
        }
        self.nodes += 1;
        self.source(span.source(), Some(span))
    }

    fn push(&mut self, item: Item<'a>, depth: usize) {
        self.stack.push((item, depth));
    }

    fn run(mut self) -> DiagnosticResult<Self> {
        while let Some((item, depth)) = self.stack.pop() {
            // Slice cursors admit one child at a time: wide rejected inputs never
            // allocate a work queue proportional to their host-owned width.
            match item {
                Item::Statements(items) => {
                    if let Some((first, rest)) = items.split_first() {
                        self.push(Item::Statements(rest), depth);
                        self.push(Item::Statement(first), depth);
                    }
                }
                Item::Expressions(items) => {
                    if let Some((first, rest)) = items.split_first() {
                        self.push(Item::Expressions(rest), depth);
                        self.push(Item::Expression(first), depth);
                    }
                }
                Item::Names(items) => {
                    if let Some((first, rest)) = items.split_first() {
                        self.push(Item::Names(rest), depth);
                        self.push(Item::Span(&first.span), depth);
                    }
                }
                Item::MapEntries(items) => {
                    if let Some(((name, value), rest)) = items.split_first() {
                        self.push(Item::MapEntries(rest), depth);
                        self.push(Item::Expression(value), depth);
                        self.push(Item::Span(&name.span), depth);
                    }
                }
                Item::Segments(items) => {
                    if let Some((first, rest)) = items.split_first() {
                        self.push(Item::Segments(rest), depth);
                        self.push(Item::Segment(first), depth);
                    }
                }
                Item::Span(span) => self.node(span, depth)?,
                Item::Block(block) => {
                    self.node(&block.span, depth)?;
                    self.push(Item::Statements(&block.statements), depth + 1);
                }
                Item::Call(call) => {
                    self.node(&call.span, depth)?;
                    self.push(Item::Expressions(&call.arguments), depth + 1);
                }
                Item::Definition(definition) => {
                    self.node(&definition.span, depth)?;
                    self.push(Item::Block(&definition.body), depth + 1);
                    self.push(Item::Names(&definition.parameters), depth + 1);
                    self.push(Item::Span(&definition.header), depth + 1);
                }
                Item::Segment(segment) => match segment {
                    AccessSegment::Literal(name) => self.node(&name.span, depth)?,
                    AccessSegment::Computed { span, index } => {
                        self.node(span, depth)?;
                        self.push(Item::Expression(index), depth + 1);
                    }
                },
                Item::Statement(statement) => {
                    self.node(&statement.span, depth)?;
                    let next = depth + 1;
                    match &statement.kind {
                        StatementKind::Assign { name, value } => {
                            self.push(
                                match value {
                                    AssignmentValue::Expression(value) => Item::Expression(value),
                                    AssignmentValue::Call(call) => Item::Call(call),
                                },
                                next,
                            );
                            self.push(Item::Span(&name.span), next);
                        }
                        StatementKind::Define(definition) => {
                            self.push(Item::Definition(definition), next)
                        }
                        StatementKind::Invoke(call) => self.push(Item::Call(call), next),
                        StatementKind::If {
                            condition,
                            then_branch,
                            else_branch,
                        } => {
                            if let Some(branch) = else_branch {
                                self.push(
                                    match branch {
                                        ElseBranch::Block(block) => Item::Block(block),
                                        ElseBranch::If(statement) => Item::Statement(statement),
                                    },
                                    next,
                                );
                            }
                            self.push(Item::Block(then_branch), next);
                            self.push(Item::Expression(condition), next);
                        }
                        StatementKind::For {
                            binding,
                            iterable,
                            body,
                        } => {
                            self.push(Item::Block(body), next);
                            self.push(Item::Expression(iterable), next);
                            self.push(Item::Span(&binding.span), next);
                        }
                        StatementKind::While { condition, body } => {
                            self.push(Item::Block(body), next);
                            self.push(Item::Expression(condition), next);
                        }
                        StatementKind::Try {
                            body,
                            binding,
                            handler,
                        } => {
                            self.push(Item::Block(handler), next);
                            if let Some(binding) = binding {
                                self.push(Item::Span(&binding.span), next);
                            }
                            self.push(Item::Block(body), next);
                        }
                        StatementKind::Return(Some(value)) => {
                            self.push(Item::Expression(value), next)
                        }
                        StatementKind::Import {
                            path_span,
                            namespace,
                            ..
                        } => {
                            self.push(Item::Span(&namespace.span), next);
                            self.push(Item::Span(path_span), next);
                        }
                        StatementKind::Return(None)
                        | StatementKind::Break
                        | StatementKind::Continue
                        | StatementKind::Rethrow => {}
                    }
                }
                Item::Expression(expression) => {
                    self.node(&expression.span, depth)?;
                    let next = depth + 1;
                    match &expression.kind {
                        ExprKind::Call(call) => self.push(Item::Call(call), next),
                        ExprKind::Access { base, segments } => {
                            self.push(Item::Segments(segments), next);
                            self.push(Item::Expression(base), next);
                        }
                        ExprKind::Array(values) => self.push(Item::Expressions(values), next),
                        ExprKind::Map(entries) => self.push(Item::MapEntries(entries), next),
                        ExprKind::Unary {
                            operator_span,
                            operand,
                            ..
                        } => {
                            self.push(Item::Expression(operand), next);
                            self.push(Item::Span(operator_span), next);
                        }
                        ExprKind::Binary {
                            operator_span,
                            left,
                            right,
                            ..
                        } => {
                            self.push(Item::Expression(right), next);
                            self.push(Item::Expression(left), next);
                            self.push(Item::Span(operator_span), next);
                        }
                        ExprKind::Integer(_)
                        | ExprKind::Float(_)
                        | ExprKind::Bool(_)
                        | ExprKind::String(_)
                        | ExprKind::Variable(_) => {}
                    }
                }
            }
        }
        Ok(self)
    }
}

pub(crate) fn check_program(
    program: &Program,
    limits: &AstLimits,
    per_source_bytes: usize,
) -> DiagnosticResult<()> {
    let mut walk = Walk::new(limits, per_source_bytes)?;
    walk.source(&program.source, None)?;
    walk.push(Item::Statements(&program.statements), 1);
    walk.run().map(|_| ())
}

pub(crate) fn check_statements(
    statements: &[Statement],
    limits: &AstLimits,
    per_source_bytes: usize,
) -> DiagnosticResult<()> {
    let mut walk = Walk::new(limits, per_source_bytes)?;
    walk.push(Item::Statements(statements), 1);
    walk.run().map(|_| ())
}

pub(crate) fn check_node(
    node: &Node,
    limits: &AstLimits,
    per_source_bytes: usize,
) -> DiagnosticResult<()> {
    let mut walk = Walk::new(limits, per_source_bytes)?;
    match node {
        Node::Statement(statement) => walk.push(Item::Statement(statement), 1),
        Node::Expression(expression) => walk.push(Item::Expression(expression), 1),
        Node::Block(block) => walk.push(Item::Block(block), 1),
        Node::None => {}
    }
    walk.run().map(|_| ())
}

pub(crate) struct DefinitionSize {
    pub(crate) nodes: usize,
    pub(crate) sources: Vec<Arc<SourceFile>>,
}

pub(crate) fn measure_definition(
    definition: &Definition,
    limits: &AstLimits,
    per_source_bytes: usize,
) -> DiagnosticResult<DefinitionSize> {
    let mut walk = Walk::new(limits, per_source_bytes)?;
    walk.retained_sources = Some(Vec::new());
    walk.push(Item::Definition(definition), 1);
    let walk = walk.run()?;
    Ok(DefinitionSize {
        nodes: walk.nodes,
        sources: walk.retained_sources.expect("enabled source inventory"),
    })
}
