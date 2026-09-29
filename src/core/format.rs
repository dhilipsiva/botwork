//! Canonical layout for scripts, suites, and dataset files.
//!
//! The formatter parses the source and rebuilds its layout from the parse tree:
//! four-space indentation, one statement per line, title-case keywords, single
//! spaces between sentence words and around binary operators, and LF line
//! endings. Literal text is copied unchanged. Comments are recovered from the
//! gaps between tokens: comments between statements keep their place, a comment
//! at the end of a line stays there, and a comment inside a statement moves to
//! the line above it. Arrays and maps written across several lines stay one item
//! per line. Before returning, the formatter parses its own output and requires
//! the same syntax tree, so formatting never changes what a program does.
use super::{
    ast::{
        suite::{Dataset, Suite},
        AccessSegment, AssignmentValue, Block, Call, ElseBranch, Expr, ExprKind, PollMode, Program,
        Statement, StatementKind,
    },
    diagnostic::{Diagnostic, DiagnosticResult},
    grammar::{BWErr, Literal},
    parser::{BWParser, Rule},
};
use pest::{iterators::Pair, Parser};
use std::{fmt::Write, path::Path};

/// Lines longer than this are wrapped between sentence words and parameters.
pub const MAX_WIDTH: usize = 100;
const INDENT: &str = "    ";

/// What a file holds, which decides its grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Script,
    Suite,
    Dataset,
}

impl SourceKind {
    /// `*.suite.botwork` holds a suite, `*.dataset.botwork` a dataset, and
    /// anything else a script.
    pub fn of_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".suite.botwork") {
            Self::Suite
        } else if name.ends_with(".dataset.botwork") {
            Self::Dataset
        } else {
            Self::Script
        }
    }
}

/// Format `source` canonically. Invalid input is rejected with the diagnostic a
/// run would report; the result always has the same syntax tree as the input.
pub fn format(name: &str, source: &str, kind: SourceKind) -> DiagnosticResult<String> {
    let before = fingerprint(name, source, kind)?;
    let output = layout(name, source, kind)?;
    let after = fingerprint(name, &output, kind).map_err(|error| internal(name, error))?;
    if before != after {
        return Err(internal(
            name,
            "the formatted syntax tree differs from the original",
        ));
    }
    Ok(output)
}

/// The canonical layout, before the meaning check.
fn layout(name: &str, source: &str, kind: SourceKind) -> DiagnosticResult<String> {
    let rule = match kind {
        SourceKind::Script => Rule::botwork,
        SourceKind::Suite => Rule::test_suite,
        SourceKind::Dataset => Rule::dataset_document,
    };
    let pairs = BWParser::parse(rule, source)
        .map_err(|error| Diagnostic::new(BWErr::ParsingError(format!("{name}: {error}"))))?;
    let formatter = Formatter { source };
    let members: Vec<_> = pairs
        .flat_map(|pair| match pair.as_rule() {
            // A suite or dataset document wraps its single declaration.
            Rule::test_suite | Rule::dataset_document => pair.into_inner().collect(),
            _ => vec![pair],
        })
        .filter(|pair| pair.as_rule() != Rule::EOI)
        .collect();
    let (_, lines) = formatter.sequence(&members, 0, source.len(), 0, true);
    let mut output = lines.join("\n");
    output.push('\n');
    if output.trim().is_empty() {
        output.clear();
    }
    Ok(output)
}

fn internal(name: &str, reason: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::new(BWErr::ParsingError(format!(
        "{name}: formatting was refused because the result would not match the original ({reason})"
    )))
}

/// A span-free rendering of what a file declares, for comparing meaning.
fn fingerprint(name: &str, source: &str, kind: SourceKind) -> DiagnosticResult<String> {
    Ok(match kind {
        SourceKind::Script => program_shape(&Program::parse_detailed(name, source)?),
        SourceKind::Suite => Suite::parse(name, source)?.fingerprint(),
        SourceKind::Dataset => dataset_shape(&Dataset::parse(name, source)?),
    })
}

pub(crate) fn dataset_shape(dataset: &Dataset) -> String {
    let mut shape = format!("{:?}", dataset.metadata());
    for row in dataset.rows() {
        let _ = write!(shape, "|{:?}=", row.metadata());
        literal_shape(row.value(), &mut shape);
    }
    shape
}

/// A value with its map keys sorted, since map order is unspecified.
fn literal_shape(value: &Literal, shape: &mut String) {
    match value {
        Literal::Array(values) => {
            shape.push('[');
            for value in values {
                literal_shape(value, shape);
                shape.push(',');
            }
            shape.push(']');
        }
        Literal::Map(entries) => {
            let mut keys: Vec<_> = entries.keys().collect();
            keys.sort();
            shape.push('{');
            for key in keys {
                let _ = write!(shape, "{key:?}:");
                literal_shape(&entries[key], shape);
                shape.push(',');
            }
            shape.push('}');
        }
        value => {
            let _ = write!(shape, "{value:?}");
        }
    }
}

pub(crate) fn program_shape(program: &Program) -> String {
    let mut shape = String::new();
    block_shape(&program.statements, &mut shape);
    shape
}

fn block_shape(statements: &[Statement], shape: &mut String) {
    shape.push('{');
    for statement in statements {
        statement_shape(statement, shape);
        shape.push(';');
    }
    shape.push('}');
}

fn statement_shape(statement: &Statement, shape: &mut String) {
    let blocks = |shape: &mut String, blocks: &[&Block]| {
        for block in blocks {
            block_shape(&block.statements, shape);
        }
    };
    match statement.kind() {
        StatementKind::Assign { name, value } => {
            let _ = write!(shape, "assign {}=", name.text);
            match value {
                AssignmentValue::Expression(expression) => expression_shape(expression, shape),
                AssignmentValue::Call(call) => call_shape(call, shape),
            }
        }
        StatementKind::Define(definition) => {
            let parameters: Vec<_> = definition
                .parameters
                .iter()
                .map(|name| &name.text)
                .collect();
            let _ = write!(shape, "define {} {parameters:?}", definition.signature);
            blocks(shape, &[&definition.body]);
        }
        StatementKind::Invoke(call) => call_shape(call, shape),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            shape.push_str("if ");
            expression_shape(condition, shape);
            blocks(shape, &[then_branch]);
            match else_branch {
                Some(ElseBranch::Block(block)) => blocks(shape, &[block]),
                Some(ElseBranch::If(nested)) => statement_shape(nested, shape),
                None => {}
            }
        }
        StatementKind::For {
            binding,
            iterable,
            body,
        } => {
            let _ = write!(shape, "for {} in ", binding.text);
            expression_shape(iterable, shape);
            blocks(shape, &[body]);
        }
        StatementKind::While { condition, body } => {
            shape.push_str("while ");
            expression_shape(condition, shape);
            blocks(shape, &[body]);
        }
        StatementKind::Poll {
            mode,
            options,
            body,
            ..
        } => {
            shape.push_str(match mode {
                PollMode::Eventually => "eventually ",
                PollMode::Retry => "retry ",
            });
            expression_shape(options, shape);
            blocks(shape, &[body]);
        }
        StatementKind::Try {
            body,
            binding,
            handler,
        } => {
            let _ = write!(shape, "try {:?}", binding.as_ref().map(|name| &name.text));
            blocks(shape, &[body, handler]);
        }
        StatementKind::Finally { body, cleanup } => {
            shape.push_str("finally");
            blocks(shape, &[body, cleanup]);
        }
        StatementKind::Return(value) => {
            shape.push_str("return ");
            if let Some(value) = value {
                expression_shape(value, shape);
            }
        }
        StatementKind::Break => shape.push_str("break"),
        StatementKind::Continue => shape.push_str("continue"),
        StatementKind::Rethrow => shape.push_str("rethrow"),
        StatementKind::Import {
            path, namespace, ..
        } => {
            let _ = write!(shape, "import {path:?} as {}", namespace.text);
        }
    }
}

fn call_shape(call: &Call, shape: &mut String) {
    let _ = write!(shape, "call {}(", call.signature);
    for argument in &call.arguments {
        expression_shape(argument, shape);
        shape.push(',');
    }
    shape.push(')');
}

fn expression_shape(expression: &Expr, shape: &mut String) {
    match &expression.kind {
        ExprKind::Integer(text) => {
            let _ = write!(shape, "int {text}");
        }
        ExprKind::Float(text) => {
            let _ = write!(shape, "float {text}");
        }
        ExprKind::Bool(value) => {
            let _ = write!(shape, "{value}");
        }
        ExprKind::String(text) => {
            let _ = write!(shape, "{text:?}");
        }
        ExprKind::Variable(name) => {
            let _ = write!(shape, "var {name}");
        }
        ExprKind::Call(call) => call_shape(call, shape),
        ExprKind::Access { base, segments } => {
            shape.push('(');
            expression_shape(base, shape);
            for segment in segments {
                match segment {
                    AccessSegment::Literal(name) => {
                        let _ = write!(shape, ".{}", name.text);
                    }
                    AccessSegment::Computed { index, .. } => {
                        shape.push('[');
                        expression_shape(index, shape);
                        shape.push(']');
                    }
                }
            }
            shape.push(')');
        }
        ExprKind::Array(values) => {
            shape.push('[');
            for value in values {
                expression_shape(value, shape);
                shape.push(',');
            }
            shape.push(']');
        }
        ExprKind::Map(entries) => {
            shape.push('{');
            for (key, value) in entries {
                let _ = write!(shape, "{:?}:", key.text);
                expression_shape(value, shape);
                shape.push(',');
            }
            shape.push('}');
        }
        ExprKind::Unary {
            operator, operand, ..
        } => {
            let _ = write!(shape, "({operator:?} ");
            expression_shape(operand, shape);
            shape.push(')');
        }
        ExprKind::Binary {
            operator,
            left,
            right,
            ..
        } => {
            let _ = write!(shape, "({operator:?} ");
            expression_shape(left, shape);
            shape.push(' ');
            expression_shape(right, shape);
            shape.push(')');
        }
    }
}

/// A comment found between tokens.
struct Comment<'s> {
    text: &'s str,
    /// A line break separates it from the previous token.
    own_line: bool,
    /// A blank line separates it from the previous token.
    blank_before: bool,
}

/// Comments in a gap between tokens, and the line breaks after the last one.
fn scan(gap: &str) -> (Vec<Comment<'_>>, usize) {
    let mut comments = Vec::new();
    let mut newlines = 0;
    let mut index = 0;
    while index < gap.len() {
        let rest = &gap[index..];
        if let Some(body) = rest.strip_prefix("###") {
            let length = body.find("###").map_or(rest.len(), |end| end + 6);
            comments.push(Comment {
                text: &rest[..length],
                own_line: newlines > 0,
                blank_before: newlines > 1,
            });
            newlines = 0;
            index += length;
        } else if rest.starts_with('#') {
            let length = rest.find(['\r', '\n']).unwrap_or(rest.len());
            comments.push(Comment {
                text: &rest[..length],
                own_line: newlines > 0,
                blank_before: newlines > 1,
            });
            newlines = 0;
            index += length;
        } else {
            if rest.starts_with('\n') {
                newlines += 1;
            }
            index += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    (comments, newlines)
}

/// The offset of the first `{` in `source[start..end]` outside comments.
fn brace(source: &str, start: usize, end: usize) -> usize {
    let mut index = start;
    while index < end {
        let rest = &source[index..end];
        if let Some(body) = rest.strip_prefix("###") {
            index += body.find("###").map_or(rest.len(), |close| close + 6);
        } else if rest.starts_with('#') {
            index += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with('{') {
            return index;
        } else {
            index += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    end
}

fn indent(level: usize) -> String {
    INDENT.repeat(level)
}

struct Formatter<'s> {
    source: &'s str,
}

impl<'s> Formatter<'s> {
    /// Lay out `members` found between `start` and `end`, one per line at
    /// `level`, with the comments around them. Returns the comments that belong
    /// on the opening line (after a `{`) and the lines. At the top of a file
    /// (`top`), every comment starts its own line.
    fn sequence(
        &self,
        members: &[Pair<'s, Rule>],
        start: usize,
        end: usize,
        level: usize,
        top: bool,
    ) -> (Vec<&'s str>, Vec<String>) {
        let mut opening = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        let mut cursor = start;
        let place = |gap: &'s str, lines: &mut Vec<String>, opening: &mut Vec<&'s str>| {
            let (comments, newlines) = scan(gap);
            for comment in comments {
                if !comment.own_line && !(top && lines.is_empty()) {
                    match lines.last_mut() {
                        Some(line) => {
                            line.push(' ');
                            line.push_str(comment.text);
                        }
                        None => opening.push(comment.text),
                    }
                    continue;
                }
                if comment.blank_before && !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!("{}{}", indent(level), comment.text));
            }
            newlines
        };
        for member in members {
            let span = member.as_span();
            let newlines = place(&self.source[cursor..span.start()], &mut lines, &mut opening);
            if newlines > 1 && lines.last().is_some_and(|line| !line.is_empty()) {
                lines.push(String::new());
            }
            let mut hoisted = Vec::new();
            self.comments(member.clone(), &mut hoisted);
            for comment in hoisted {
                lines.push(format!("{}{comment}", indent(level)));
            }
            lines.push(format!(
                "{}{}",
                indent(level),
                self.member(member.clone(), level)
            ));
            cursor = span.end();
        }
        place(&self.source[cursor..end], &mut lines, &mut opening);
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        (opening, lines)
    }

    /// Comments inside a member, outside its nested sequences, in order.
    fn comments(&self, pair: Pair<'s, Rule>, found: &mut Vec<&'s str>) {
        match pair.as_rule() {
            // Blocks lay out their own comments; strings hold none.
            Rule::stmt_block | Rule::string | Rule::string_content => return,
            Rule::dataset_inline | Rule::suite_definition => {
                // The header's comments; the body's belong to its sequence.
                let span = pair.as_span();
                let mut cursor = span.start();
                for child in pair.into_inner().take_while(|child| {
                    matches!(
                        child.as_rule(),
                        Rule::suite_id | Rule::suite_name | Rule::suite_tags
                    )
                }) {
                    let child_span = child.as_span();
                    found.extend(
                        scan(&self.source[cursor..child_span.start()])
                            .0
                            .iter()
                            .map(|comment| comment.text),
                    );
                    self.comments(child, found);
                    cursor = child_span.end();
                }
                let open = brace(self.source, cursor, span.end());
                found.extend(
                    scan(&self.source[cursor..open])
                        .0
                        .iter()
                        .map(|comment| comment.text),
                );
                return;
            }
            _ => {}
        }
        let span = pair.as_span();
        let mut cursor = span.start();
        for child in pair.into_inner() {
            let child_span = child.as_span();
            found.extend(
                scan(&self.source[cursor..child_span.start()])
                    .0
                    .iter()
                    .map(|comment| comment.text),
            );
            self.comments(child, found);
            cursor = child_span.end();
        }
        found.extend(
            scan(&self.source[cursor..span.end()])
                .0
                .iter()
                .map(|comment| comment.text),
        );
    }

    fn member(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        match pair.as_rule() {
            Rule::suite_definition => self.suite(pair, level),
            Rule::suite_dataset => self.member(pair.into_inner().next().expect("dataset"), level),
            Rule::dataset_inline => self.dataset(pair, level),
            Rule::dataset_external => {
                let mut children = pair.into_inner();
                let id = self.id(children.next().expect("id"));
                let mut text = format!("Dataset {id} From ");
                for child in children {
                    match child.as_rule() {
                        Rule::dataset_format => {
                            text.push_str(&child.as_str().to_uppercase());
                            text.push(' ');
                        }
                        _ => text.push_str(&self.id(child)),
                    }
                }
                text
            }
            Rule::dataset_row => {
                let mut text = String::from("Row");
                for child in pair.into_inner() {
                    match child.as_rule() {
                        Rule::suite_id => {
                            text.push(' ');
                            text.push_str(&self.id(child));
                        }
                        Rule::suite_name | Rule::suite_tags => {
                            text.push(' ');
                            text.push_str(&self.metadata(child));
                        }
                        _ => {
                            text.push_str(" Values |");
                            text.push_str(&self.data(child, level));
                            text.push('|');
                        }
                    }
                }
                text
            }
            Rule::suite_library => {
                format!(
                    "Library {}",
                    self.block(pair.into_inner().next().expect("block"), level)
                )
            }
            Rule::suite_fixture => {
                let mut children = pair.into_inner();
                let keyword = match children.next().expect("fixture").as_rule() {
                    Rule::suite_setup => "SuiteSetup",
                    Rule::suite_teardown => "SuiteTeardown",
                    Rule::case_setup => "CaseSetup",
                    _ => "CaseTeardown",
                };
                format!(
                    "{keyword} {}",
                    self.block(children.next().expect("block"), level)
                )
            }
            Rule::suite_case => {
                let mut text = String::from("Case");
                for child in pair.into_inner() {
                    text.push(' ');
                    match child.as_rule() {
                        Rule::suite_id => text.push_str(&self.id(child)),
                        Rule::suite_name | Rule::suite_tags => text.push_str(&self.metadata(child)),
                        Rule::suite_using => {
                            let mut parts = child.into_inner();
                            let dataset = self.id(parts.next().expect("dataset"));
                            let binding = parts.next().expect("binding").as_str();
                            let _ = write!(text, "Using {dataset} As |{binding}|");
                        }
                        _ => text.push_str(&self.block(child, level)),
                    }
                }
                text
            }
            _ => self.statement(pair, level),
        }
    }

    fn id(&self, pair: Pair<'s, Rule>) -> String {
        format!("|{}|", pair.into_inner().next().expect("string").as_str())
    }

    fn metadata(&self, pair: Pair<'s, Rule>) -> String {
        match pair.as_rule() {
            Rule::suite_name => format!("Named {}", self.id(pair.into_inner().next().expect("id"))),
            _ => {
                let tags: Vec<_> = pair.into_inner().map(|tag| tag.as_str()).collect();
                format!("Tags |[{}]|", tags.join(", "))
            }
        }
    }

    fn suite(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let span = pair.as_span();
        let children: Vec<_> = pair.into_inner().collect();
        let mut text = String::from("Suite");
        let mut cursor = span.start();
        let mut members = Vec::new();
        for child in children {
            match child.as_rule() {
                Rule::suite_id | Rule::suite_name | Rule::suite_tags if members.is_empty() => {
                    text.push(' ');
                    text.push_str(&match child.as_rule() {
                        Rule::suite_id => self.id(child.clone()),
                        _ => self.metadata(child.clone()),
                    });
                    cursor = child.as_span().end();
                }
                _ => members.push(child),
            }
        }
        let open = brace(self.source, cursor, span.end());
        text.push(' ');
        text.push_str(&self.body(&members, open + 1, span.end() - 1, level));
        text
    }

    fn dataset(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let span = pair.as_span();
        let mut text = String::from("Dataset");
        let mut cursor = span.start();
        let mut rows = Vec::new();
        for child in pair.into_inner() {
            match child.as_rule() {
                Rule::dataset_row => rows.push(child),
                Rule::suite_id => {
                    cursor = child.as_span().end();
                    text.push(' ');
                    text.push_str(&self.id(child));
                }
                _ => {
                    cursor = child.as_span().end();
                    text.push(' ');
                    text.push_str(&self.metadata(child));
                }
            }
        }
        let open = brace(self.source, cursor, span.end());
        text.push(' ');
        text.push_str(&self.body(&rows, open + 1, span.end() - 1, level));
        text
    }

    /// `{`, the members one level deeper, and `}` at this level.
    fn body(&self, members: &[Pair<'s, Rule>], start: usize, end: usize, level: usize) -> String {
        let (opening, lines) = self.sequence(members, start, end, level + 1, false);
        if opening.is_empty() && lines.is_empty() {
            return "{}".into();
        }
        let mut text = String::from("{");
        for comment in opening {
            text.push(' ');
            text.push_str(comment);
        }
        for line in lines {
            text.push('\n');
            text.push_str(&line);
        }
        text.push('\n');
        text.push_str(&indent(level));
        text.push('}');
        text
    }

    fn block(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let span = pair.as_span();
        let members: Vec<_> = pair.into_inner().collect();
        self.body(&members, span.start() + 1, span.end() - 1, level)
    }

    fn statement(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let rule = pair.as_rule();
        if rule == Rule::stmt_invoke {
            // A whole-line call; calls inside expressions are never wrapped.
            return self.sentence(pair, level, 1);
        }
        let mut children = pair
            .into_inner()
            .filter(|child| child.as_rule() != Rule::continuation);
        let mut next = || children.next().expect("statement part");
        match rule {
            Rule::stmt_break => "Break".into(),
            Rule::stmt_continue => "Continue".into(),
            Rule::stmt_rethrow => "Rethrow".into(),
            Rule::stmt_return => match children.next() {
                Some(value) => format!("Return {}", self.parameter(value, level)),
                None => "Return".into(),
            },
            Rule::stmt_import => {
                let path = next().as_str();
                format!("Import |{path}| As |{}|", next().as_str())
            }
            Rule::stmt_assign => {
                let name = next().as_str();
                let value = next();
                let value = match value.as_rule() {
                    Rule::stmt_invoke => self.sentence(value, level, 0),
                    _ => self.parameter(value, level),
                };
                format!("|{name}| = {value}")
            }
            Rule::stmt_if => {
                let condition = self.parameter(next(), level);
                let mut text = format!("If {condition} {}", self.block(next(), level));
                if let Some(branch) = children.next() {
                    let branch = branch.into_inner().next().expect("else branch");
                    text.push_str(" Else ");
                    text.push_str(&match branch.as_rule() {
                        Rule::stmt_block => self.block(branch, level),
                        _ => self.statement(branch, level),
                    });
                }
                text
            }
            Rule::stmt_for => {
                let binding = next().as_str();
                let iterable = self.parameter(next(), level);
                format!(
                    "For |{binding}| In {iterable} {}",
                    self.block(next(), level)
                )
            }
            Rule::stmt_while | Rule::stmt_eventually | Rule::stmt_retry => {
                let keyword = match rule {
                    Rule::stmt_while => "While",
                    Rule::stmt_eventually => "Eventually",
                    _ => "Retry",
                };
                let condition = self.parameter(next(), level);
                format!("{keyword} {condition} {}", self.block(next(), level))
            }
            Rule::stmt_try => {
                let mut text = format!("Try {}", self.block(next(), level));
                for clause in children {
                    let keyword = if clause.as_rule() == Rule::stmt_catch {
                        "Catch"
                    } else {
                        "Finally"
                    };
                    let _ = write!(text, " {keyword}");
                    for part in clause.into_inner() {
                        match part.as_rule() {
                            Rule::ident => {
                                let _ = write!(text, " |{}|", part.as_str());
                            }
                            _ => {
                                text.push(' ');
                                text.push_str(&self.block(part, level));
                            }
                        }
                    }
                }
                text
            }
            Rule::stmt_define => {
                let header = self.sentence(next(), level, 2);
                format!("{header} {}", self.block(next(), level))
            }
            _ => unreachable!("statement rule {rule:?}"),
        }
    }

    /// Sentence words and parameters separated by single spaces. At statement
    /// level (`wrap` > 0), a line past [`MAX_WIDTH`] continues with `\` between
    /// words; `wrap` is the width the rest of the line needs.
    fn sentence(&self, pair: Pair<'s, Rule>, level: usize, wrap: usize) -> String {
        let mut tokens = Vec::new();
        for child in pair.into_inner() {
            match child.as_rule() {
                Rule::part => {
                    let words: Vec<_> = child
                        .as_str()
                        .split([' ', '\t'])
                        .filter(|word| !word.is_empty())
                        .collect();
                    if !words.is_empty() {
                        tokens.push(words.join(" "));
                    }
                }
                Rule::param_invoke => tokens.push(self.parameter(child, level)),
                Rule::ident => tokens.push(format!("|{}|", child.as_str())),
                _ => {}
            }
        }
        let line = tokens.join(" ");
        let width = level * INDENT.len() + line.chars().count() + if wrap == 2 { 2 } else { 0 };
        // Wrapping only helps when every word or parameter fits on a
        // continuation line of its own.
        let fits = tokens
            .iter()
            .all(|token| (level + 1) * INDENT.len() + token.chars().count() + 2 <= MAX_WIDTH);
        if wrap == 0 || width <= MAX_WIDTH || line.contains('\n') || tokens.len() < 2 || !fits {
            return line;
        }
        let continuation = indent(level + 1);
        let mut text = String::new();
        let mut column = level * INDENT.len();
        for (index, token) in tokens.iter().enumerate() {
            let length = token.chars().count();
            if index > 0 {
                if column + 1 + length + 2 > MAX_WIDTH {
                    text.push_str(" \\\n");
                    text.push_str(&continuation);
                    column = continuation.len();
                } else {
                    text.push(' ');
                    column += 1;
                }
            }
            text.push_str(token);
            column += length;
        }
        text
    }

    fn parameter(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let expression = pair.into_inner().next().expect("expression");
        format!("|{}|", self.expression(expression, level))
    }

    fn expression(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let parts: Vec<_> = pair
            .into_inner()
            .map(|child| match child.as_rule() {
                Rule::unary | Rule::power => self.operand(child, level),
                _ => child.as_str().to_owned(),
            })
            .collect();
        parts.join(" ")
    }

    fn operand(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        match pair.as_rule() {
            Rule::unary => {
                let mut children = pair.into_inner();
                let operator = children.next().expect("operator").as_str();
                format!(
                    "{operator}{}",
                    self.operand(children.next().expect("operand"), level)
                )
            }
            Rule::power => {
                let mut children = pair.into_inner();
                let base = self.primary(children.next().expect("base"), level);
                match (children.next(), children.next()) {
                    (Some(_), Some(exponent)) => {
                        format!("{base} ^ {}", self.operand(exponent, level))
                    }
                    _ => base,
                }
            }
            _ => self.primary(pair, level),
        }
    }

    fn primary(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let mut text = String::new();
        for child in pair.into_inner() {
            match child.as_rule() {
                Rule::call_expression => {
                    let call = child.into_inner().next().expect("call");
                    let _ = write!(text, "@{{ {} }}", self.sentence(call, level, 0));
                }
                Rule::braced_expression => {
                    let inner = child.into_inner().next().expect("expression");
                    let _ = write!(text, "({})", self.expression(inner, level));
                }
                Rule::named_access => {
                    let _ = write!(
                        text,
                        ".{}",
                        child.into_inner().next().expect("name").as_str()
                    );
                }
                Rule::computed_access => {
                    let inner = child.into_inner().next().expect("expression");
                    let _ = write!(text, "[{}]", self.expression(inner, level));
                }
                Rule::array | Rule::map => text.push_str(&self.collection(child, level)),
                _ => text.push_str(child.as_str()),
            }
        }
        text
    }

    /// An array or map: on one line, or one item per line when written across
    /// lines.
    fn collection(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        let (open, close) = match pair.as_rule() {
            Rule::array | Rule::dataset_array => ("[", "]"),
            _ => ("{", "}"),
        };
        let multiline = pair.as_str().contains('\n');
        let items: Vec<String> = pair
            .into_inner()
            .map(|item| match item.as_rule() {
                Rule::map_pair | Rule::dataset_entry => {
                    let mut parts = item.into_inner();
                    let key = parts.next().expect("key").as_str();
                    let value = parts.next().expect("value");
                    let value = if matches!(value.as_rule(), Rule::expression) {
                        self.expression(value, level + usize::from(multiline))
                    } else {
                        self.data(value, level + usize::from(multiline))
                    };
                    format!("{key}: {value}")
                }
                Rule::expression => self.expression(item, level + usize::from(multiline)),
                _ => self.data(item, level + usize::from(multiline)),
            })
            .collect();
        if items.is_empty() {
            return format!("{open}{close}");
        }
        if !multiline {
            return format!("{open}{}{close}", items.join(", "));
        }
        let mut text = String::from(open);
        for item in items {
            let _ = write!(text, "\n{}{item},", indent(level + 1));
        }
        let _ = write!(text, "\n{}{close}", indent(level));
        text
    }

    /// A dataset value: a literal without expressions, which may use `none`.
    fn data(&self, pair: Pair<'s, Rule>, level: usize) -> String {
        match pair.as_rule() {
            Rule::dataset_map | Rule::dataset_array => self.collection(pair, level),
            Rule::dataset_number => pair
                .into_inner()
                .map(|part| part.as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => pair.as_str().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests;
