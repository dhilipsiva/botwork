//! Explicit suite discovery, stable case identity, and immutable case programs.
use super::*;
use crate::core::{grammar::Literal, value_limits::ValueLimits};
use std::collections::{BTreeSet, HashSet};

mod dataset;
mod fixtures;
pub use dataset::{
    Dataset, DatasetDefinition, DatasetFormat, Row, MAX_DATASETS, MAX_DATASET_PATH_BYTES,
    MAX_DATASET_ROWS, MAX_DATA_NODES, MAX_DATA_ROWS,
};
pub use fixtures::FixturePrograms;

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
    parameters: Option<Parameters>,
}

#[derive(Clone, Debug)]
struct Parameters {
    dataset: usize,
    binding: Name,
}

impl Case {
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn span(&self) -> &Span {
        &self.body.span
    }
    /// The variable a parameterized case binds its dataset row to.
    pub fn binding(&self) -> Option<&str> {
        self.parameters
            .as_ref()
            .map(|parameters| parameters.binding.text.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct Suite {
    metadata: Metadata,
    source: Arc<SourceFile>,
    library: Vec<Statement>,
    datasets: Vec<DatasetDefinition>,
    cases: Vec<Case>,
    fixtures: fixtures::Fixtures,
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

/// A runnable identity is a suite/case, optionally followed by a stable row ID.
pub fn valid_run_id(id: &str) -> bool {
    let mut parts = id.split('/');
    matches!((parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(suite), Some(case), row, None) if valid_id(suite) && valid_id(case) && row.is_none_or(valid_id))
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
        let mut datasets = Vec::new();
        let mut dataset_ids = HashMap::new();
        let mut data_budget = dataset::DataBudget::default();
        let mut fixtures = fixtures::Fixtures::default();
        for pair in inner {
            let span = Span::of(&pair, &source);
            if pair.as_rule() == Rule::suite_dataset {
                if datasets.len() == MAX_DATASETS {
                    return Err(resource("dataset definitions", MAX_DATASETS).at(&span));
                }
                let definition = DatasetDefinition::lower(pair, &source, &mut data_budget)?;
                if dataset_ids
                    .insert(definition.id.clone(), datasets.len())
                    .is_some()
                {
                    return Err(
                        configuration(format!("Duplicate dataset ID {:?}", definition.id))
                            .at(&span),
                    );
                }
                datasets.push(definition);
            } else if pair.as_rule() == Rule::suite_library {
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
            } else if pair.as_rule() == Rule::suite_fixture {
                fixtures.lower(pair, &source)?;
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
                let parameters = if inner
                    .peek()
                    .is_some_and(|pair| pair.as_rule() == Rule::suite_using)
                {
                    let pair = inner.next().expect("case dataset reference");
                    let span = Span::of(&pair, &source);
                    let mut children = pair.into_inner();
                    let id = self::text(
                        children
                            .next()
                            .expect("dataset ID")
                            .into_inner()
                            .next()
                            .expect("ID string"),
                        &source,
                        MAX_ID_BYTES,
                    )?;
                    let dataset = *dataset_ids.get(&id).ok_or_else(|| {
                        configuration(format!("Unknown dataset ID {id:?}")).at(&span)
                    })?;
                    let binding = lower_name(children.next().expect("row binding"), &source);
                    if binding.text.len() > MAX_ID_BYTES {
                        return Err(resource("row binding bytes", MAX_ID_BYTES).at(&binding.span));
                    }
                    Some(Parameters { dataset, binding })
                } else {
                    None
                };
                let body = block(inner.next().expect("case body"), &source)
                    .map_err(|error| error.at(Some(&span)).default_diagnostic())?;
                cases.push(Case {
                    metadata: case_metadata,
                    body,
                    parameters,
                });
            }
        }
        let groups = || {
            std::iter::once(library.as_slice())
                .chain(fixtures.statements())
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
            datasets,
            cases,
            fixtures,
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

    pub fn datasets(&self) -> &[DatasetDefinition] {
        &self.datasets
    }

    /// Load declared external datasets explicitly. The caller chooses filesystem,
    /// cache and I/O policy; no case body or library declaration is evaluated.
    pub fn resolve_datasets(
        mut self,
        mut load: impl FnMut(&str, &Span, DatasetFormat) -> DiagnosticResult<Arc<Dataset>>,
    ) -> DiagnosticResult<Self> {
        for definition in &mut self.datasets {
            if definition.data.is_none() {
                let format = definition.format();
                let (path, span) = definition.external().expect("external dataset declaration");
                definition.data = Some(load(path, span, format)?);
            }
        }
        self.run_count()?;
        Ok(self)
    }

    /// Number of independent case/row executions, before selection.
    pub fn run_count(&self) -> DiagnosticResult<usize> {
        if self
            .datasets
            .iter()
            .any(|definition| definition.data.is_none())
        {
            return Err(configuration(
                "External datasets must be resolved before selecting cases",
            ));
        }
        let mut count = 0;
        for case in &self.cases {
            count += case.parameters.as_ref().map_or(1, |parameters| {
                self.datasets[parameters.dataset]
                    .data
                    .as_ref()
                    .expect("resolved dataset")
                    .rows()
                    .len()
            });
            if count > MAX_SELECTED_CASES {
                return Err(resource("discovered case rows", MAX_SELECTED_CASES));
            }
        }
        Ok(count)
    }

    /// Compose only the admitted case. Definitions/source owners stay shared;
    /// the Engine or Context still creates fresh mutable invocation/module state.
    pub fn program(&self, index: usize) -> Option<Program> {
        let case = self.cases.get(index)?;
        let statements = self.fixtures.case_statements(&self.library, &case.body);
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
    row: Option<usize>,
}

impl SelectedCase {
    pub fn shared_suite(&self) -> Arc<Suite> {
        Arc::clone(&self.suite)
    }
    pub fn suite(&self) -> &Suite {
        &self.suite
    }
    pub fn case(&self) -> &Case {
        &self.suite.cases[self.index]
    }
    pub fn id(&self) -> String {
        let mut id = self.case_id();
        if let Some(row) = self.row() {
            id.push('/');
            id.push_str(row.metadata().id());
        }
        id
    }
    pub fn case_id(&self) -> String {
        format!("{}/{}", self.suite.metadata.id, self.case().metadata.id)
    }
    pub fn dataset(&self) -> Option<&DatasetDefinition> {
        self.case()
            .parameters
            .as_ref()
            .map(|parameters| &self.suite.datasets[parameters.dataset])
    }
    pub fn row(&self) -> Option<&Row> {
        self.row.map(|index| {
            &self
                .dataset()
                .expect("row dataset")
                .data
                .as_ref()
                .expect("resolved dataset")
                .rows()[index]
        })
    }
    pub fn display_name(&self) -> String {
        self.row().map_or_else(
            || self.case().metadata.name.clone(),
            |row| format!("{} / {}", self.case().metadata.name, row.metadata().name()),
        )
    }
    /// Copy only this admitted row after checking its value limit. The case's
    /// explicit row binding overrides a common input with the same name.
    pub fn bind_inputs(
        &self,
        mut inputs: std::collections::BTreeMap<String, Literal>,
        limits: &ValueLimits,
    ) -> DiagnosticResult<std::collections::BTreeMap<String, Literal>> {
        if let Some(row) = self.row() {
            limits
                .check(row.value())
                .map_err(|error| Diagnostic::new(error).at(row.span()))?;
            let parameters = self.case().parameters.as_ref().expect("row binding");
            inputs.insert(parameters.binding.text.clone(), row.value().clone());
        }
        Ok(inputs)
    }
    pub fn tags(&self) -> BTreeSet<&str> {
        let mut tags: BTreeSet<_> = self
            .suite
            .metadata
            .tags
            .iter()
            .chain(self.case().metadata.tags.iter())
            .map(String::as_str)
            .collect();
        if let Some(dataset) = self.dataset() {
            tags.extend(
                dataset
                    .data
                    .as_ref()
                    .expect("resolved dataset")
                    .metadata()
                    .tags()
                    .iter()
                    .map(String::as_str),
            );
        }
        if let Some(row) = self.row() {
            tags.extend(row.metadata().tags().iter().map(String::as_str));
        }
        tags
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
        let mut sources = HashSet::new();
        let mut data_seen = HashSet::new();
        let mut data_rows = 0;
        let mut data_nodes = 0;
        let mut data_count = 0;
        let mut declared = HashSet::new();
        for suite in suites {
            if !suite_ids.insert(suite.metadata.id.as_str()) {
                return Err(configuration(format!(
                    "Duplicate suite ID {:?}",
                    suite.metadata.id
                )));
            }
            suite.run_count()?;
            if sources.insert(Arc::as_ptr(&suite.source)) {
                source_bytes += suite.source.text().len();
            }
            for definition in &suite.datasets {
                let data = definition.data.as_ref().expect("resolved dataset");
                if data_seen.insert(Arc::as_ptr(data)) {
                    data_count += 1;
                    data_rows += data.rows().len();
                    data_nodes += data.value_nodes();
                    if data_count > MAX_DATASETS {
                        return Err(resource("discovered datasets", MAX_DATASETS));
                    }
                    if data_rows > MAX_DATA_ROWS {
                        return Err(resource("defined data rows", MAX_DATA_ROWS));
                    }
                    if data_nodes > MAX_DATA_NODES {
                        return Err(resource("dataset literal nodes", MAX_DATA_NODES));
                    }
                }
                if sources.insert(Arc::as_ptr(data.source())) {
                    source_bytes += data.source().text().len();
                }
            }
            if source_bytes > MAX_SUITE_SOURCE_BYTES {
                return Err(resource("suite source bytes", MAX_SUITE_SOURCE_BYTES));
            }
            for index in 0..suite.cases.len() {
                declared.insert(format!(
                    "{}/{}",
                    suite.metadata.id, suite.cases[index].metadata.id
                ));
                let rows = suite.cases[index].parameters.as_ref().map(|parameters| {
                    suite.datasets[parameters.dataset]
                        .data
                        .as_ref()
                        .expect("resolved dataset")
                        .rows()
                        .len()
                });
                for row in 0..rows.unwrap_or(1) {
                    if all.len() == MAX_SELECTED_CASES {
                        return Err(resource("discovered cases", MAX_SELECTED_CASES));
                    }
                    all.push(SelectedCase {
                        suite: Arc::clone(suite),
                        index,
                        row: rows.map(|_| row),
                    });
                }
            }
        }
        let known: HashSet<_> = all.iter().map(SelectedCase::id).collect();
        for (values, allow_template) in std::iter::once((&self.cases, true))
            .chain(self.failed.as_ref().map(|values| (values, false)))
        {
            if values.len() > MAX_SELECTED_CASES {
                return Err(resource("case selectors", MAX_SELECTED_CASES));
            }
            for id in values {
                if !valid_run_id(id) {
                    return Err(configuration(
                        "Case selectors must contain suite/case or suite/case/row IDs",
                    ));
                }
                if !known.contains(id) && !(allow_template && declared.contains(id)) {
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
            (cases.is_empty() || cases.contains(&id) || cases.contains(&case.case_id()))
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
