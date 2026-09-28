//! Explicit suite discovery, stable case identity, and immutable case programs.
use super::*;
use std::collections::{BTreeSet, HashSet};

#[cfg(test)]
mod tests;

pub const MAX_SUITE_CASES: usize = 1024;
pub const MAX_SUITES: usize = 64;
pub const MAX_SELECTED_CASES: usize = 4096;
pub const MAX_SUITE_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_TAGS: usize = 32;
pub const MAX_NAME_BYTES: usize = 512;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Metadata {
    id: String,
    name: String,
    tags: BTreeSet<String>,
}

impl Metadata {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn tags(&self) -> &BTreeSet<String> {
        &self.tags
    }
}

#[derive(Clone, Debug)]
pub struct Case {
    metadata: Metadata,
    body: Block,
}

impl Case {
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn span(&self) -> &Span {
        &self.body.span
    }
}

#[derive(Clone, Debug)]
pub struct Suite {
    metadata: Metadata,
    source: Arc<SourceFile>,
    library: Vec<Statement>,
    cases: Vec<Case>,
}

pub fn configuration(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(BWErr::RunConfiguration(message.into()))
}

pub fn resource(resource: &'static str, limit: usize) -> Diagnostic {
    Diagnostic::new(BWErr::ResourceLimit {
        resource,
        limit: limit as u64,
    })
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

pub fn valid_case_id(id: &str) -> bool {
    id.split_once('/')
        .is_some_and(|(suite, case)| valid_id(suite) && valid_id(case))
}

fn text(pair: Pair<Rule>, source: &Arc<SourceFile>, maximum: usize) -> DiagnosticResult<String> {
    let span = Span::of(&pair, source);
    let value = decode_string(pair).map_err(|error| error.at(Some(&span)).default_diagnostic())?;
    if value.is_empty() || value.len() > maximum {
        return Err(configuration(format!(
            "Suite metadata must contain 1–{maximum} UTF-8 bytes"
        ))
        .at(&span));
    }
    Ok(value)
}

fn metadata(
    inner: &mut pest::iterators::Pairs<'_, Rule>,
    source: &Arc<SourceFile>,
) -> DiagnosticResult<Metadata> {
    let id = inner.next().expect("suite/case ID from grammar");
    let span = Span::of(&id, source);
    let id = text(
        id.into_inner().next().expect("ID string"),
        source,
        MAX_ID_BYTES,
    )?;
    if !valid_id(&id) {
        return Err(configuration("Suite/case IDs must start with an ASCII letter or digit and contain only letters, digits, '.', '_', or '-'").at(&span));
    }
    let mut result = Metadata {
        name: id.clone(),
        id,
        tags: BTreeSet::new(),
    };
    while let Some(pair) = inner.peek() {
        match pair.as_rule() {
            Rule::suite_name => {
                let name = inner
                    .next()
                    .unwrap()
                    .into_inner()
                    .next()
                    .unwrap()
                    .into_inner()
                    .next()
                    .unwrap();
                result.name = text(name, source, MAX_NAME_BYTES)?;
            }
            Rule::suite_tags => {
                for pair in inner.next().unwrap().into_inner() {
                    if result.tags.len() == MAX_TAGS {
                        return Err(resource("suite tags", MAX_TAGS).at(&Span::of(&pair, source)));
                    }
                    let span = Span::of(&pair, source);
                    let tag = text(pair, source, MAX_ID_BYTES)?;
                    if tag.chars().any(char::is_control) || !result.tags.insert(tag) {
                        return Err(configuration(
                            "Tags must be distinct strings without control characters",
                        )
                        .at(&span));
                    }
                }
            }
            _ => break,
        }
    }
    Ok(result)
}

impl Suite {
    /// Discover declarations without evaluating imports, definitions, or case bodies.
    pub fn parse(name: &str, text: &str) -> DiagnosticResult<Self> {
        check_source(name, text, DEFAULT_SOURCE_BYTES, &SyntaxLimits::default())?;
        let source = Arc::new(SourceFile::from_owned_parts(name.into(), text.into()));
        let pair = super::super::parser::BWParser::parse(Rule::test_suite, text)
            .map_err(|error| parse_error(error, &source, |failure| failure.default_diagnostic()))?
            .next()
            .expect("suite root")
            .into_inner()
            .next()
            .expect("suite definition");
        let mut inner = pair.into_inner();
        let metadata = metadata(&mut inner, &source)?;
        let mut library = Vec::new();
        let mut cases = Vec::new();
        let mut ids = HashSet::new();
        for pair in inner {
            let span = Span::of(&pair, &source);
            if pair.as_rule() == Rule::suite_library {
                library = block(pair.into_inner().next().unwrap(), &source)
                    .map_err(|error| error.at(Some(&span)).default_diagnostic())?
                    .statements;
                for statement in &library {
                    if !matches!(
                        statement.kind(),
                        StatementKind::Define(_) | StatementKind::Import { .. }
                    ) {
                        return Err(configuration(
                            "Library blocks may contain only custom definitions and imports",
                        )
                        .at(&statement.span));
                    }
                }
            } else {
                if cases.len() == MAX_SUITE_CASES {
                    return Err(resource("suite cases", MAX_SUITE_CASES).at(&span));
                }
                let mut inner = pair.into_inner();
                let case_metadata = self::metadata(&mut inner, &source)?;
                if !ids.insert(case_metadata.id.clone()) {
                    return Err(
                        configuration(format!("Duplicate case ID {:?}", case_metadata.id))
                            .at(&span),
                    );
                }
                let body = block(inner.next().expect("case body"), &source)
                    .map_err(|error| error.at(Some(&span)).default_diagnostic())?;
                cases.push(Case {
                    metadata: case_metadata,
                    body,
                });
            }
        }
        let groups = || {
            std::iter::once(library.as_slice())
                .chain(cases.iter().map(|case| case.body.statements.as_slice()))
        };
        ast_limits::check_suite(&source, groups()).map_err(AstFailure::default_diagnostic)?;
        for statements in groups() {
            validate_control_script(statements).map_err(AstFailure::default_diagnostic)?;
        }
        Ok(Self {
            metadata,
            source,
            library,
            cases,
        })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn cases(&self) -> &[Case] {
        &self.cases
    }
    pub fn source(&self) -> &Arc<SourceFile> {
        &self.source
    }

    /// Compose only the admitted case. Definitions/source owners stay shared;
    /// the Engine or Context still creates fresh mutable invocation/module state.
    pub fn program(&self, index: usize) -> Option<Program> {
        let case = self.cases.get(index)?;
        let statements = self
            .library
            .iter()
            .chain(&case.body.statements)
            .cloned()
            .collect();
        Some(Program {
            source: Arc::clone(&self.source),
            statements,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SelectedCase {
    suite: Arc<Suite>,
    index: usize,
}

impl SelectedCase {
    pub fn suite(&self) -> &Suite {
        &self.suite
    }
    pub fn case(&self) -> &Case {
        &self.suite.cases[self.index]
    }
    pub fn id(&self) -> String {
        format!("{}/{}", self.suite.metadata.id, self.case().metadata.id)
    }
    pub fn tags(&self) -> BTreeSet<&str> {
        self.suite
            .metadata
            .tags
            .iter()
            .chain(self.case().metadata.tags.iter())
            .map(String::as_str)
            .collect()
    }
    pub fn program(&self) -> Program {
        self.suite
            .program(self.index)
            .expect("validated selected case")
    }
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub cases: Vec<String>,
    pub tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    /// None means all otherwise-selected cases; Some(empty) means no prior failures.
    pub failed: Option<Vec<String>>,
}

impl Selection {
    /// Preserve supplied suite order and declaration order; selection never reorders cases.
    pub fn select(&self, suites: &[Arc<Suite>]) -> DiagnosticResult<Vec<SelectedCase>> {
        if suites.len() > MAX_SUITES {
            return Err(resource("suites", MAX_SUITES));
        }
        let mut suite_ids = HashSet::new();
        let mut all = Vec::new();
        let mut source_bytes = 0;
        for suite in suites {
            if !suite_ids.insert(suite.metadata.id.as_str()) {
                return Err(configuration(format!(
                    "Duplicate suite ID {:?}",
                    suite.metadata.id
                )));
            }
            source_bytes += suite.source.text().len();
            if source_bytes > MAX_SUITE_SOURCE_BYTES {
                return Err(resource("suite source bytes", MAX_SUITE_SOURCE_BYTES));
            }
            for index in 0..suite.cases.len() {
                if all.len() == MAX_SELECTED_CASES {
                    return Err(resource("discovered cases", MAX_SELECTED_CASES));
                }
                all.push(SelectedCase {
                    suite: Arc::clone(suite),
                    index,
                });
            }
        }
        let known: HashSet<_> = all.iter().map(SelectedCase::id).collect();
        for values in std::iter::once(&self.cases).chain(self.failed.as_ref()) {
            if values.len() > MAX_SELECTED_CASES {
                return Err(resource("case selectors", MAX_SELECTED_CASES));
            }
            for id in values {
                if !valid_case_id(id) {
                    return Err(configuration(
                        "Case selectors must contain two valid IDs separated by '/'",
                    ));
                }
                if !known.contains(id) {
                    return Err(configuration(format!("Unknown case selector {id:?}")));
                }
            }
        }
        for tags in [&self.tags, &self.exclude_tags] {
            if tags.len() > MAX_TAGS {
                return Err(resource("tag selectors", MAX_TAGS));
            }
            if tags.iter().any(|tag| {
                tag.is_empty() || tag.len() > MAX_ID_BYTES || tag.chars().any(char::is_control)
            }) {
                return Err(configuration(
                    "Tag selectors must contain 1–128 UTF-8 bytes without control characters",
                ));
            }
        }
        let cases: HashSet<_> = self.cases.iter().collect();
        let failed = self
            .failed
            .as_ref()
            .map(|values| values.iter().collect::<HashSet<_>>());
        all.retain(|case| {
            let id = case.id();
            let tags = case.tags();
            (cases.is_empty() || cases.contains(&id))
                && failed.as_ref().is_none_or(|failed| failed.contains(&id))
                && (self.tags.is_empty() || self.tags.iter().any(|tag| tags.contains(tag.as_str())))
                && !self
                    .exclude_tags
                    .iter()
                    .any(|tag| tags.contains(tag.as_str()))
        });
        if all.is_empty() && !self.failed.as_ref().is_some_and(Vec::is_empty) {
            return Err(configuration("No cases match the selection"));
        }
        Ok(all)
    }
}
