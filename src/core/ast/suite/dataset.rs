//! Immutable, literal-only datasets. External resolution belongs to the caller.
use super::*;
use crate::core::{grammar::Literal, value_limits::ValueLimits};

#[cfg(test)]
mod tests;

pub const MAX_DATASETS: usize = 64;
pub const MAX_DATASET_ROWS: usize = 1024;
pub const MAX_DATA_ROWS: usize = 4096;
pub const MAX_DATA_NODES: usize = 65_536;
pub const MAX_DATASET_PATH_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub struct Row {
    metadata: Metadata,
    value: Literal,
    span: Span,
}

impl Row {
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn value(&self) -> &Literal {
        &self.value
    }
    pub fn span(&self) -> &Span {
        &self.span
    }
}

#[derive(Clone, Debug)]
pub struct Dataset {
    metadata: Metadata,
    source: Arc<SourceFile>,
    rows: Vec<Row>,
    nodes: usize,
}

impl Dataset {
    /// Parse a standalone dataset document without evaluating code or loading files.
    pub fn parse(name: &str, text: &str) -> DiagnosticResult<Self> {
        check_source(name, text, DEFAULT_SOURCE_BYTES, &SyntaxLimits::default())?;
        let source = Arc::new(SourceFile::from_owned_parts(name.into(), text.into()));
        let pair = BWParser::parse(Rule::dataset_document, text)
            .map_err(|error| parse_error(error, &source, |failure| failure.default_diagnostic()))?
            .next()
            .expect("dataset document")
            .into_inner()
            .next()
            .expect("dataset definition");
        Self::lower(pair, &source, &mut DataBudget::default())
    }

    pub(super) fn lower(
        pair: Pair<Rule>,
        source: &Arc<SourceFile>,
        budget: &mut DataBudget,
    ) -> DiagnosticResult<Self> {
        let start_nodes = budget.nodes;
        let mut inner = pair.into_inner();
        let metadata = metadata(&mut inner, source)?;
        let mut rows = Vec::new();
        let mut ids = HashSet::new();
        for pair in inner {
            let span = Span::of(&pair, source);
            if rows.len() == MAX_DATASET_ROWS {
                return Err(resource("dataset rows", MAX_DATASET_ROWS).at(&span));
            }
            if budget.rows == MAX_DATA_ROWS {
                return Err(resource("defined data rows", MAX_DATA_ROWS).at(&span));
            }
            budget.rows += 1;
            let mut inner = pair.into_inner();
            let metadata = super::metadata(&mut inner, source)?;
            if !ids.insert(metadata.id.clone()) {
                return Err(
                    configuration(format!("Duplicate dataset row ID {:?}", metadata.id)).at(&span),
                );
            }
            let value = lower_value(inner.next().expect("row values"), source, budget, 1)?;
            ValueLimits::default()
                .check(&value)
                .map_err(|error| Diagnostic::new(error).at(&span))?;
            rows.push(Row {
                metadata,
                value,
                span,
            });
        }
        Ok(Self {
            metadata,
            source: Arc::clone(source),
            rows,
            nodes: budget.nodes - start_nodes,
        })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn source(&self) -> &Arc<SourceFile> {
        &self.source
    }
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
    /// Literal nodes parsed, including overwritten duplicate map entries.
    pub fn value_nodes(&self) -> usize {
        self.nodes
    }
}

#[derive(Clone, Debug)]
pub struct DatasetDefinition {
    pub(super) id: String,
    pub(super) data: Option<Arc<Dataset>>,
    external: Option<(String, Span)>,
}

impl DatasetDefinition {
    pub(super) fn lower(
        pair: Pair<Rule>,
        source: &Arc<SourceFile>,
        budget: &mut DataBudget,
    ) -> DiagnosticResult<Self> {
        let pair = pair.into_inner().next().expect("dataset declaration");
        if pair.as_rule() == Rule::dataset_inline {
            let data = Arc::new(Dataset::lower(pair, source, budget)?);
            return Ok(Self {
                id: data.metadata.id.clone(),
                data: Some(data),
                external: None,
            });
        }
        let mut inner = pair.into_inner();
        let id = metadata(&mut inner, source)?.id;
        let path = inner.next().expect("dataset path");
        let span = Span::of(&path, source);
        let path = text(
            path.into_inner().next().expect("path string"),
            source,
            MAX_DATASET_PATH_BYTES,
        )?;
        if path.contains('\0') {
            return Err(configuration("Dataset paths cannot contain NUL").at(&span));
        }
        Ok(Self {
            id,
            data: None,
            external: Some((path, span)),
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn data(&self) -> Option<&Arc<Dataset>> {
        self.data.as_ref()
    }
    pub fn external(&self) -> Option<(&str, &Span)> {
        self.external
            .as_ref()
            .map(|(path, span)| (path.as_str(), span))
    }
}

#[derive(Default)]
pub(super) struct DataBudget {
    nodes: usize,
    rows: usize,
}

fn lower_value(
    pair: Pair<Rule>,
    source: &Arc<SourceFile>,
    budget: &mut DataBudget,
    depth: usize,
) -> DiagnosticResult<Literal> {
    let span = Span::of(&pair, source);
    if budget.nodes == MAX_DATA_NODES {
        return Err(resource("dataset literal nodes", MAX_DATA_NODES).at(&span));
    }
    let limits = ValueLimits::default();
    if depth > limits.depth {
        return Err(resource("dataset value depth", limits.depth).at(&span));
    }
    budget.nodes += 1;
    let value = match pair.as_rule() {
        Rule::dataset_none => Literal::None,
        Rule::boolean_true => Literal::Bool(true),
        Rule::boolean_false => Literal::Bool(false),
        Rule::string => Literal::String(
            decode_string(pair).map_err(|error| error.at(Some(&span)).default_diagnostic())?,
        ),
        Rule::dataset_number => {
            let mut inner = pair.into_inner();
            let first = inner.next().expect("number");
            let (negative, number) = if first.as_rule() == Rule::minus {
                (true, inner.next().expect("magnitude"))
            } else {
                (false, first)
            };
            let text = format!("{}{}", if negative { "-" } else { "" }, number.as_str());
            if number.as_rule() == Rule::integer {
                Literal::Int(text.parse().map_err(|_| {
                    configuration("Dataset integers must fit signed 32-bit values").at(&span)
                })?)
            } else {
                let value: f32 = text
                    .parse()
                    .map_err(|_| configuration("Invalid dataset float").at(&span))?;
                if !value.is_finite() {
                    return Err(configuration("Dataset floats must be finite").at(&span));
                }
                Literal::Float(value)
            }
        }
        Rule::dataset_array => {
            let mut values = Vec::new();
            for child in pair.into_inner() {
                limits
                    .container_header(values.len() + 1)
                    .map_err(|error| Diagnostic::new(error).at(&span))?;
                values.push(lower_value(child, source, budget, depth + 1)?);
            }
            Literal::Array(values)
        }
        Rule::dataset_map => {
            let mut values = HashMap::new();
            for entry in pair.into_inner() {
                let mut inner = entry.into_inner();
                let key = inner.next().expect("map key");
                let key = if key.as_rule() == Rule::string {
                    decode_string(key)
                        .map_err(|error| error.at(Some(&span)).default_diagnostic())?
                } else {
                    key.as_str().into()
                };
                limits
                    .key_size(key.len())
                    .map_err(|error| Diagnostic::new(error).at(&span))?;
                if !values.contains_key(&key) {
                    limits
                        .container_header(values.len() + 1)
                        .map_err(|error| Diagnostic::new(error).at(&span))?;
                }
                let value =
                    lower_value(inner.next().expect("map value"), source, budget, depth + 1)?;
                values.insert(key, value);
            }
            Literal::Map(values)
        }
        _ => unreachable!("literal-only dataset grammar"),
    };
    Ok(value)
}
