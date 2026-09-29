//! Editor support for one document: diagnostics, completion, hover,
//! go-to-definition, and references. Diagnostics come from the same parser and
//! [`Analyzer`] as `--check`, with the same codes and spans, including the
//! problems it finds in imported modules; navigation comes from a symbol index
//! that resolves names the way the runtime does. Offsets are UTF-8 byte offsets
//! into the text.
use super::{
    analysis::{module_path, Analyzer, Severity},
    ast::{
        suite::{Dataset, Suite},
        AccessSegment, AssignmentValue, Block, Call, ElseBranch, Expr, ExprKind, Name, Program,
        Span, Statement, StatementKind,
    },
    diagnostic::Diagnostic,
    eval::Context,
    format::SourceKind,
    run::RunLimits,
    signature::StatementSignature,
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// A problem to show in the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    /// The runtime code (such as `BW2002`) or, for a finding without one, its rule.
    pub code: String,
    pub message: String,
    /// How to fix it, as `--check` prints it.
    pub help: String,
}

/// A place in a file and a byte range. The file is a path, or the analyzed
/// document's name; an empty file means the analyzed document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    pub file: String,
    pub start: usize,
    pub end: usize,
}

/// Hover text, in Markdown, for the symbol covering a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    pub start: usize,
    pub end: usize,
    pub markdown: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionKind {
    Keyword,
    Statement,
    Variable,
}

/// A completion: the text to insert and what it is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Completion {
    pub kind: CompletionKind,
    pub label: String,
    pub detail: String,
}

/// Keywords that may start a statement.
const KEYWORDS: [&str; 12] = [
    "If",
    "For",
    "While",
    "Try",
    "Return",
    "Break",
    "Continue",
    "Rethrow",
    "Import",
    "Eventually",
    "Retry",
    "Else",
];

/// Problems in a file the analysis reached, such as an imported module, and
/// the text they were found in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileProblems {
    pub text: String,
    pub problems: Vec<Problem>,
}

/// The analysis of one document.
#[derive(Debug, Default)]
pub struct Analysis {
    pub problems: Vec<Problem>,
    /// Problems in other files, such as imported modules, by path.
    pub related: BTreeMap<String, FileProblems>,
    /// Whether the document parsed; navigation needs a parsed document.
    pub parsed: bool,
    index: Index,
}

/// Builds analyses against one statement catalogue and working directory.
pub struct Language {
    analyzer: Analyzer,
    builtins: HashMap<String, StatementSignature>,
    directory: PathBuf,
}

impl Language {
    /// The CLI's built-in statements, with imports resolved from `directory`.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        let mut context =
            Context::with_limits(RunLimits::default()).expect("default limits are valid");
        context.init_statements();
        let builtins = context
            .statement_signatures()
            .into_iter()
            .map(|signature| (signature.normalized().to_owned(), signature.clone()))
            .collect();
        Self {
            analyzer: Analyzer::default().with_modules(&directory),
            builtins,
            directory,
        }
    }

    /// Analyze `text`, named `name` (a file path, which locates its imports).
    pub fn analyze(&self, name: &str, text: &str, kind: SourceKind) -> Analysis {
        let mut analysis = Analysis::default();
        let parsed = match kind {
            SourceKind::Script => Program::parse_detailed(name, text).map(|program| {
                let report = self.analyzer.report_program(&program);
                (vec![program], report)
            }),
            SourceKind::Suite => Suite::parse(name, text).map(|suite| {
                let report = self.analyzer.report_suite(&suite);
                let fixtures = suite.fixture_programs();
                let mut programs = vec![fixtures.setup, fixtures.teardown];
                programs.extend((0..suite.cases().len()).filter_map(|index| suite.program(index)));
                (programs, report)
            }),
            SourceKind::Dataset => Dataset::parse(name, text).map(|_| (vec![], Default::default())),
        };
        match parsed {
            Ok((programs, report)) => {
                analysis.parsed = true;
                for finding in &report.findings {
                    let problem = Problem {
                        start: finding.span.start(),
                        end: finding.span.end(),
                        severity: finding.severity(),
                        code: finding.rule.code().map_or_else(
                            || finding.rule.as_str().to_owned(),
                            |code| code.to_string(),
                        ),
                        message: format!("{} ({})", finding.message, finding.rule.as_str()),
                        help: finding.help.clone(),
                    };
                    analysis.place(name, Some(&finding.span), problem);
                }
                // Modules that failed to parse, as `--check` reports them.
                for diagnostic in &report.diagnostics {
                    analysis.place(name, diagnostic.span.as_ref(), problem(diagnostic));
                }
                let mut builder = IndexBuilder {
                    language: self,
                    index: Index::default(),
                    frames: 0,
                    seen: BTreeSet::new(),
                };
                for program in &programs {
                    builder.program(program);
                }
                analysis.index = builder.index;
            }
            Err(diagnostic) => analysis.problems.push(problem(&diagnostic)),
        }
        analysis
    }

    fn help(&self, signature: &str) -> Option<String> {
        self.builtins
            .get(signature)
            .map(|builtin| format!("```\n{}\n```", builtin.display_help()))
    }
}

/// A parse or validation error at its span, or at the start when it has none.
fn problem(diagnostic: &Diagnostic) -> Problem {
    let (start, end) = diagnostic
        .span
        .as_ref()
        .map_or((0, 0), |span| (span.start(), span.end()));
    Problem {
        start,
        end,
        severity: Severity::Error,
        code: diagnostic.code().to_string(),
        message: diagnostic.error.to_string(),
        help: diagnostic.help(),
    }
}

#[derive(Clone, Debug)]
struct DefinitionSymbol {
    header: String,
    file: String,
    header_start: usize,
    header_end: usize,
}

#[derive(Clone, Debug)]
struct CallSymbol {
    signature: String,
    span: (usize, usize),
    /// The definition it reaches: in this file's index or a module's.
    target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Builtin,
    Definition(usize),
    Module(Location, String),
    Unknown,
}

#[derive(Clone, Debug)]
struct VariableSymbol {
    name: String,
    span: (usize, usize),
    /// The frame whose binding it reaches, if any.
    frame: Option<usize>,
    binding: bool,
}

#[derive(Clone, Debug)]
struct ImportSymbol {
    path_span: (usize, usize),
    module: Option<PathBuf>,
}

#[derive(Debug, Default)]
struct Index {
    definitions: Vec<DefinitionSymbol>,
    calls: Vec<CallSymbol>,
    variables: Vec<VariableSymbol>,
    imports: Vec<ImportSymbol>,
    /// Names bound anywhere, for completion.
    names: BTreeSet<String>,
}

/// One frame's names while indexing: definitions and imports by position, and
/// the variables it binds.
struct Frame<'a> {
    id: usize,
    parent: Option<&'a Frame<'a>>,
    definitions: HashMap<String, Vec<(usize, usize)>>,
    variables: BTreeSet<String>,
    modules: HashMap<String, Vec<(usize, Option<PathBuf>)>>,
}

impl Frame<'_> {
    fn chain(&self) -> impl Iterator<Item = &Frame<'_>> {
        std::iter::successors(Some(self), |frame| frame.parent)
    }
}

struct IndexBuilder<'l> {
    language: &'l Language,
    index: Index,
    frames: usize,
    /// Case programs repeat the library; index each span once.
    seen: BTreeSet<(usize, usize, u8)>,
}

impl IndexBuilder<'_> {
    fn program(&mut self, program: &Program) {
        let frame = self.frame(None, &program.statements, &[]);
        self.block(&program.statements, &frame);
    }

    fn frame<'p>(
        &mut self,
        parent: Option<&'p Frame<'p>>,
        statements: &[Statement],
        parameters: &[Name],
    ) -> Frame<'p> {
        let id = self.frames;
        self.frames += 1;
        let mut frame = Frame {
            id,
            parent,
            definitions: HashMap::new(),
            variables: parameters.iter().map(|name| name.text.clone()).collect(),
            modules: HashMap::new(),
        };
        for parameter in parameters {
            self.variable(&parameter.text, &parameter.span, Some(id), true);
        }
        self.collect(statements, &mut frame);
        frame
    }

    /// Record a frame's definitions, bindings, and imports, as the analyzer does.
    fn collect(&mut self, statements: &[Statement], frame: &mut Frame<'_>) {
        for statement in statements {
            match statement.kind() {
                StatementKind::Assign { name, .. } => {
                    frame.variables.insert(name.text.clone());
                }
                StatementKind::Define(definition) => {
                    let header = &definition.header;
                    if self.seen.insert((header.start(), header.end(), 0)) {
                        self.index.definitions.push(DefinitionSymbol {
                            header: header.text().trim().to_owned(),
                            file: header.source().name().to_owned(),
                            header_start: header.start(),
                            header_end: header.end(),
                        });
                    }
                    let index = self
                        .index
                        .definitions
                        .iter()
                        .position(|symbol| symbol.header_start == header.start())
                        .expect("indexed definition");
                    frame
                        .definitions
                        .entry(definition.signature.clone())
                        .or_default()
                        .push((statement.span.start(), index));
                }
                StatementKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.collect(&then_branch.statements, frame);
                    match else_branch {
                        Some(ElseBranch::Block(block)) => self.collect(&block.statements, frame),
                        Some(ElseBranch::If(nested)) => {
                            self.collect(std::slice::from_ref(nested.as_ref()), frame)
                        }
                        None => {}
                    }
                }
                StatementKind::For { binding, body, .. } => {
                    frame.variables.insert(binding.text.clone());
                    self.collect(&body.statements, frame);
                }
                StatementKind::While { body, .. } | StatementKind::Poll { body, .. } => {
                    self.collect(&body.statements, frame)
                }
                StatementKind::Try {
                    body,
                    binding,
                    handler,
                } => {
                    if let Some(binding) = binding {
                        frame.variables.insert(binding.text.clone());
                    }
                    self.collect(&body.statements, frame);
                    self.collect(&handler.statements, frame);
                }
                StatementKind::Finally { body, cleanup } => {
                    self.collect(&body.statements, frame);
                    self.collect(&cleanup.statements, frame);
                }
                StatementKind::Import {
                    path,
                    path_span,
                    namespace,
                } => {
                    let module =
                        module_path(path_span.source().name(), &self.language.directory, path)
                            .and_then(|requested| fs::canonicalize(requested).ok());
                    if self.seen.insert((path_span.start(), path_span.end(), 1)) {
                        self.index.imports.push(ImportSymbol {
                            path_span: (path_span.start(), path_span.end()),
                            module: module.clone(),
                        });
                    }
                    frame
                        .modules
                        .entry(super::ast::normalize_sentence(&namespace.text))
                        .or_default()
                        .push((statement.span.start(), module));
                }
                StatementKind::Invoke(_)
                | StatementKind::Return(_)
                | StatementKind::Break
                | StatementKind::Continue
                | StatementKind::Rethrow => {}
            }
        }
    }

    fn variable(&mut self, name: &str, span: &Span, frame: Option<usize>, binding: bool) {
        self.index.names.insert(name.to_owned());
        if self.seen.insert((span.start(), span.end(), 2)) {
            self.index.variables.push(VariableSymbol {
                name: name.to_owned(),
                span: (span.start(), span.end()),
                frame,
                binding,
            });
        }
    }

    /// The frame whose binding a name reaches.
    fn binding(frame: &Frame<'_>, name: &str) -> Option<usize> {
        frame
            .chain()
            .find(|frame| frame.variables.contains(name))
            .map(|frame| frame.id)
    }

    fn block(&mut self, statements: &[Statement], frame: &Frame<'_>) {
        for statement in statements {
            self.statement(statement, frame);
        }
    }

    fn blocks(&mut self, blocks: &[&Block], frame: &Frame<'_>) {
        for block in blocks {
            self.block(&block.statements, frame);
        }
    }

    fn statement(&mut self, statement: &Statement, frame: &Frame<'_>) {
        match statement.kind() {
            StatementKind::Assign { name, value } => {
                self.variable(&name.text, &name.span, Some(frame.id), true);
                match value {
                    AssignmentValue::Expression(expression) => self.expression(expression, frame),
                    AssignmentValue::Call(call) => self.call(call, frame),
                }
            }
            StatementKind::Define(definition) => {
                let child = self.frame(
                    Some(frame),
                    &definition.body.statements,
                    &definition.parameters,
                );
                self.block(&definition.body.statements, &child);
            }
            StatementKind::Invoke(call) => self.call(call, frame),
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expression(condition, frame);
                self.blocks(&[then_branch], frame);
                match else_branch {
                    Some(ElseBranch::Block(block)) => self.blocks(&[block], frame),
                    Some(ElseBranch::If(nested)) => self.statement(nested, frame),
                    None => {}
                }
            }
            StatementKind::For {
                binding,
                iterable,
                body,
            } => {
                self.variable(&binding.text, &binding.span, Some(frame.id), true);
                self.expression(iterable, frame);
                self.blocks(&[body], frame);
            }
            StatementKind::While { condition, body } => {
                self.expression(condition, frame);
                self.blocks(&[body], frame);
            }
            StatementKind::Poll { options, body, .. } => {
                self.expression(options, frame);
                self.blocks(&[body], frame);
            }
            StatementKind::Try {
                body,
                binding,
                handler,
            } => {
                if let Some(binding) = binding {
                    self.variable(&binding.text, &binding.span, Some(frame.id), true);
                }
                self.blocks(&[body, handler], frame);
            }
            StatementKind::Finally { body, cleanup } => self.blocks(&[body, cleanup], frame),
            StatementKind::Return(Some(expression)) => self.expression(expression, frame),
            StatementKind::Return(None)
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Rethrow
            | StatementKind::Import { .. } => {}
        }
    }

    fn expression(&mut self, expression: &Expr, frame: &Frame<'_>) {
        match &expression.kind {
            ExprKind::Variable(name) => {
                let binding = Self::binding(frame, name);
                self.variable(name, &expression.span, binding, false);
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
        let target = match signature.split_once("::") {
            Some((namespace, exported)) => frame
                .chain()
                .find_map(|frame| frame.modules.get(namespace))
                .and_then(|modules| modules.iter().find_map(|(_, module)| module.clone()))
                .and_then(|module| module_definition(&module, exported, 0))
                .map_or(Target::Unknown, |(location, header)| {
                    Target::Module(location, header)
                }),
            None => {
                let own = frame.definitions.get(signature).and_then(|definitions| {
                    definitions
                        .iter()
                        .find(|(start, _)| *start < call.span.start())
                        .map(|(_, index)| *index)
                });
                let outer = || {
                    frame
                        .chain()
                        .skip(1)
                        .find_map(|frame| frame.definitions.get(signature))
                        .map(|definitions| definitions[0].1)
                };
                match own.or_else(outer) {
                    Some(index) => Target::Definition(index),
                    None if self.language.builtins.contains_key(signature) => Target::Builtin,
                    None => Target::Unknown,
                }
            }
        };
        if self.seen.insert((call.span.start(), call.span.end(), 3)) {
            self.index.calls.push(CallSymbol {
                signature: signature.to_owned(),
                span: (call.span.start(), call.span.end()),
                target,
            });
        }
    }
}

/// Where a module defines `exported`, following re-exports a few levels deep.
fn module_definition(module: &Path, exported: &str, depth: usize) -> Option<(Location, String)> {
    if depth > 8 {
        return None;
    }
    let mut text = String::new();
    fs::File::open(module)
        .ok()?
        .take(DEFAULT_SOURCE_BYTES as u64)
        .read_to_string(&mut text)
        .ok()?;
    let name = module.to_string_lossy().into_owned();
    let program = Program::parse_detailed(&name, &text).ok()?;
    if let Some((namespace, rest)) = exported.split_once("::") {
        let import = program
            .statements
            .iter()
            .find_map(|statement| match statement.kind() {
                StatementKind::Import {
                    path,
                    namespace: alias,
                    ..
                } if super::ast::normalize_sentence(&alias.text) == namespace => Some(path.clone()),
                _ => None,
            })?;
        let requested = module_path(&name, Path::new("/"), &import)?;
        return module_definition(&fs::canonicalize(requested).ok()?, rest, depth + 1);
    }
    program
        .statements
        .iter()
        .find_map(|statement| match statement.kind() {
            StatementKind::Define(definition) if definition.signature == exported => Some((
                Location {
                    file: name.clone(),
                    start: definition.header.start(),
                    end: definition.header.end(),
                },
                definition.header.text().trim().to_owned(),
            )),
            _ => None,
        })
}

impl Analysis {
    /// Keep a problem with the document named `name`, or with the other file
    /// its span is in.
    fn place(&mut self, name: &str, span: Option<&Span>, problem: Problem) {
        match span.map(|span| span.source()) {
            Some(source) if source.name() != name => self
                .related
                .entry(source.name().to_owned())
                .or_insert_with(|| FileProblems {
                    text: source.text().to_owned(),
                    problems: Vec::new(),
                })
                .problems
                .push(problem),
            _ => self.problems.push(problem),
        }
    }

    fn call_at(&self, offset: usize) -> Option<&CallSymbol> {
        // The innermost call covering the offset: calls nest inside arguments.
        self.index
            .calls
            .iter()
            .filter(|call| call.span.0 <= offset && offset < call.span.1)
            .min_by_key(|call| call.span.1 - call.span.0)
    }

    fn variable_at(&self, offset: usize) -> Option<&VariableSymbol> {
        self.index
            .variables
            .iter()
            .find(|variable| variable.span.0 <= offset && offset < variable.span.1)
    }

    fn definition_at(&self, offset: usize) -> Option<usize> {
        self.index.definitions.iter().position(|definition| {
            definition.header_start <= offset && offset < definition.header_end
        })
    }

    fn definition_location(&self, index: usize) -> Location {
        let definition = &self.index.definitions[index];
        Location {
            file: definition.file.clone(),
            start: definition.header_start,
            end: definition.header_end,
        }
    }

    /// Markdown for the symbol at `offset`.
    pub fn hover(&self, language: &Language, offset: usize) -> Option<Hover> {
        if let Some(variable) = self.variable_at(offset) {
            let text = match variable.frame {
                Some(_) => format!("```botwork\n|{}|\n```\nVariable", variable.name),
                None => format!(
                    "```botwork\n|{}|\n```\nNever assigned here: an input variable",
                    variable.name
                ),
            };
            return Some(Hover {
                start: variable.span.0,
                end: variable.span.1,
                markdown: text,
            });
        }
        if let Some(index) = self.definition_at(offset) {
            let definition = &self.index.definitions[index];
            return Some(Hover {
                start: definition.header_start,
                end: definition.header_end,
                markdown: format!(
                    "```botwork\n{}\n```\nDefined in this file",
                    definition.header
                ),
            });
        }
        let call = self.call_at(offset)?;
        let markdown = match &call.target {
            Target::Builtin => language.help(&call.signature)?,
            Target::Definition(index) => {
                let definition = &self.index.definitions[*index];
                format!(
                    "```botwork\n{}\n```\nDefined in this file",
                    definition.header
                )
            }
            Target::Module(location, header) => {
                format!("```botwork\n{header}\n```\nDefined in `{}`", location.file)
            }
            Target::Unknown => format!("`{}` is not defined", call.signature),
        };
        Some(Hover {
            start: call.span.0,
            end: call.span.1,
            markdown,
        })
    }

    /// Where the symbol at `offset` is defined.
    pub fn definition(&self, offset: usize) -> Vec<Location> {
        if let Some(import) = self
            .index
            .imports
            .iter()
            .find(|import| import.path_span.0 <= offset && offset < import.path_span.1)
        {
            return import
                .module
                .iter()
                .map(|module| Location {
                    file: module.to_string_lossy().into_owned(),
                    start: 0,
                    end: 0,
                })
                .collect();
        }
        if let Some(variable) = self.variable_at(offset) {
            return self
                .variable_references(variable)
                .into_iter()
                .filter(|(_, binding)| *binding)
                .take(1)
                .map(|(location, _)| location)
                .collect();
        }
        if let Some(index) = self.definition_at(offset) {
            return vec![self.definition_location(index)];
        }
        match self.call_at(offset).map(|call| &call.target) {
            Some(Target::Definition(index)) => vec![self.definition_location(*index)],
            Some(Target::Module(location, _)) => vec![location.clone()],
            _ => vec![],
        }
    }

    fn variable_references(&self, variable: &VariableSymbol) -> Vec<(Location, bool)> {
        let Some(frame) = variable.frame else {
            return vec![];
        };
        let mut found: Vec<_> = self
            .index
            .variables
            .iter()
            .filter(|other| other.name == variable.name && other.frame == Some(frame))
            .map(|other| {
                (
                    Location {
                        file: String::new(),
                        start: other.span.0,
                        end: other.span.1,
                    },
                    other.binding,
                )
            })
            .collect();
        found.sort();
        found
    }

    /// Every use of the symbol at `offset` in this file, and its definition when
    /// `declaration` is set. Locations in this file carry an empty file name.
    pub fn references(&self, offset: usize, declaration: bool) -> Vec<Location> {
        let here = |start: usize, end: usize| Location {
            file: String::new(),
            start,
            end,
        };
        if let Some(variable) = self.variable_at(offset) {
            return self
                .variable_references(variable)
                .into_iter()
                .filter(|(_, binding)| declaration || !binding)
                .map(|(location, _)| here(location.start, location.end))
                .collect();
        }
        let target = match self.definition_at(offset) {
            Some(index) => Target::Definition(index),
            None => match self.call_at(offset) {
                Some(call) if call.target == Target::Builtin || call.target == Target::Unknown => {
                    let signature = call.signature.clone();
                    return self
                        .index
                        .calls
                        .iter()
                        .filter(|other| other.signature == signature)
                        .map(|other| here(other.span.0, other.span.1))
                        .collect();
                }
                Some(call) => call.target.clone(),
                None => return vec![],
            },
        };
        let mut found: Vec<Location> = self
            .index
            .calls
            .iter()
            .filter(|call| call.target == target)
            .map(|call| here(call.span.0, call.span.1))
            .collect();
        if let (true, Target::Definition(index)) = (declaration, &target) {
            let definition = &self.index.definitions[*index];
            found.push(here(definition.header_start, definition.header_end));
        }
        found.sort();
        found
    }

    /// Completions at `offset` in `text`: variables inside a parameter,
    /// statements and keywords elsewhere.
    pub fn completions(&self, language: &Language, text: &str, offset: usize) -> Vec<Completion> {
        let mut completions = BTreeMap::new();
        if inside_parameter(text, offset) {
            for name in &self.index.names {
                completions.insert(
                    (CompletionKind::Variable, name.clone()),
                    "variable".to_owned(),
                );
            }
            for value in ["true", "false"] {
                completions.insert(
                    (CompletionKind::Keyword, value.to_owned()),
                    "boolean".to_owned(),
                );
            }
        } else {
            for keyword in KEYWORDS {
                completions.insert(
                    (CompletionKind::Keyword, keyword.to_owned()),
                    "keyword".to_owned(),
                );
            }
            for builtin in language.builtins.values() {
                completions.insert(
                    (
                        CompletionKind::Statement,
                        builtin.header().text().trim().to_owned(),
                    ),
                    builtin.documentation().to_owned(),
                );
            }
            for definition in &self.index.definitions {
                completions.insert(
                    (CompletionKind::Statement, definition.header.clone()),
                    "defined in this file".to_owned(),
                );
            }
        }
        completions
            .into_iter()
            .map(|((kind, label), detail)| Completion {
                kind,
                label,
                detail,
            })
            .collect()
    }
}

/// Whether `offset` is inside a `|...|` parameter of its line, ignoring pipes
/// inside strings and comments.
fn inside_parameter(text: &str, offset: usize) -> bool {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let mut inside = false;
    let mut string = false;
    let mut escaped = false;
    for character in text[start..offset].chars() {
        match character {
            _ if escaped => escaped = false,
            '\\' if string => escaped = true,
            '"' => string = !string,
            '#' if !string => return inside,
            '|' if !string => inside = !inside,
            _ => {}
        }
    }
    inside
}

#[cfg(test)]
mod tests;
