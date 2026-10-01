//! Static checks over parsed scripts and suites. Nothing is executed: no
//! statement runs, no module initializes, and no host operation is called.
//! With [`Analyzer::with_modules`], imported local modules are read and checked
//! too; otherwise calls into them are left unchecked.
//!
//! Each finding names a stable rule. A finding that predicts a runtime error
//! carries that error's diagnostic code, so `--check` and a failing run agree.
//! Name resolution mirrors the runtime: definitions register when they execute,
//! definition bodies resolve names when they are called, and nested scopes may
//! shadow built-ins while the root scope may not.
use super::{
    ast::{
        normalize_sentence, AccessSegment, AssignmentValue, Block, Call, ElseBranch, Expr,
        ExprKind, Program, Span, Statement, StatementKind, UnaryOp,
    },
    diagnostic::{Diagnostic, DiagnosticCode},
    eval::Context,
    run::RunLimits,
    signature::{StatementSignature, ValueKind},
    suite::Suite,
    syntax_limits::DEFAULT_SOURCE_BYTES,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt, fs,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
};

/// How serious a finding is. Errors predict a failing run; warnings flag code
/// that is likely wrong but may be intended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
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
#[non_exhaustive]
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
    /// An import that cannot load: not a local `.botwork` path, or unreadable.
    ImportFailure,
    /// An import that reaches a module already being imported.
    ImportCycle,
    /// A second import under an alias already imported in the same scope.
    DuplicateNamespace,
}

impl Rule {
    pub const ALL: [Self; 10] = [
        Self::UndefinedStatement,
        Self::StatementBeforeDefinition,
        Self::UndefinedVariable,
        Self::DuplicateStatement,
        Self::UnreachableCode,
        Self::ArgumentKind,
        Self::ConditionKind,
        Self::ImportFailure,
        Self::ImportCycle,
        Self::DuplicateNamespace,
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
            Self::ImportFailure => "import-failure",
            Self::ImportCycle => "import-cycle",
            Self::DuplicateNamespace => "duplicate-namespace",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::UndefinedStatement
            | Self::DuplicateStatement
            | Self::ArgumentKind
            | Self::ConditionKind
            | Self::ImportFailure
            | Self::ImportCycle
            | Self::DuplicateNamespace => Severity::Error,
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
            Self::ImportFailure => Some(DiagnosticCode::ImportRead),
            Self::ImportCycle => Some(DiagnosticCode::ImportCycle),
            Self::DuplicateNamespace => Some(DiagnosticCode::DuplicateNamespace),
            Self::UnreachableCode => None,
        }
    }
}

/// One problem found without running the program.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
            crate::core::diagnostic::shown_path(self.span.source().name()),
            self.severity().as_str(),
            self.rule.as_str()
        )?;
        if let Some(code) = self.rule.code() {
            write!(output, "[{code}] ")?;
        }
        write!(output, "{}\n  help: {}", self.message, self.help)
    }
}

/// What a built-in statement's result depends on outside the script. The check
/// cannot know these results without running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum External {
    Files,
    Environment,
    Processes,
    Network,
    Clock,
}

impl External {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Environment => "the environment",
            Self::Processes => "processes",
            Self::Network => "the network",
            Self::Clock => "the clock",
        }
    }
}

/// Built-ins whose results depend on the outside world. Lexical path statements,
/// the operating system name, and the path separator are fixed per build.
pub const EXTERNAL_STATEMENTS: [(&str, External); 33] = [
    ("Absolute Path |path|", External::Files),
    ("Append To File |path| Text |text|", External::Files),
    ("Canonical Path |path|", External::Files),
    ("Copy File |source| To |destination|", External::Files),
    ("Create Directory |path|", External::Files),
    ("Create File |path| Text |text|", External::Files),
    (
        "Create Temporary Directory In |directory| Prefix |prefix|",
        External::Files,
    ),
    ("Directory Exists |path|", External::Files),
    ("File Exists |path|", External::Files),
    ("File Size |path|", External::Files),
    ("List Directory |path|", External::Files),
    ("Move Path |source| To |destination|", External::Files),
    ("Path Exists |path|", External::Files),
    ("Path Kind |path|", External::Files),
    ("Read Binary File |path|", External::Files),
    ("Read File |path|", External::Files),
    (
        "Remove Directory |path| Recursively |recursive|",
        External::Files,
    ),
    ("Remove File |path|", External::Files),
    ("Working Directory", External::Files),
    ("Write Binary File |path| Bytes |bytes|", External::Files),
    ("Write File |path| Text |text|", External::Files),
    ("Environment Variable Exists |name|", External::Environment),
    ("Environment Variables", External::Environment),
    ("Get Environment Variable |name|", External::Environment),
    (
        "Run Process |executable| With Arguments |arguments|",
        External::Processes,
    ),
    (
        "Run Process |executable| With Arguments |arguments| Options |options|",
        External::Processes,
    ),
    (
        "Run Binary Process |executable| With Arguments |arguments|",
        External::Processes,
    ),
    (
        "Run Binary Process |executable| With Arguments |arguments| Options |options|",
        External::Processes,
    ),
    ("HTTP Request |method| To |url|", External::Network),
    (
        "HTTP Request |method| To |url| Options |options|",
        External::Network,
    ),
    ("HTTP Binary Request |method| To |url|", External::Network),
    (
        "HTTP Binary Request |method| To |url| Options |options|",
        External::Network,
    ),
    ("Current Date Time In |zone|", External::Clock),
];

/// Everything a check established, and what it could not.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Report {
    /// Findings, the checked file's first and then each module's, in source order.
    pub findings: Vec<Finding>,
    /// Modules that failed to parse or validate, as the run would report them.
    pub diagnostics: Vec<Diagnostic>,
    /// Variables read but never assigned: they must arrive as input variables.
    pub inputs: BTreeSet<String>,
    /// Calls to built-ins whose results depend on the outside world.
    pub external: BTreeMap<External, usize>,
    /// Imported local modules that were read and checked, by canonical path.
    pub modules: BTreeSet<PathBuf>,
    /// Calls into imported modules that were not checked.
    pub unchecked_imports: usize,
}

impl Report {
    /// Findings of error severity plus modules that failed to parse.
    pub fn errors(&self) -> usize {
        self.diagnostics.len()
            + self
                .findings
                .iter()
                .filter(|finding| finding.severity() == Severity::Error)
                .count()
    }

    pub fn warnings(&self) -> usize {
        self.findings.len() - (self.errors() - self.diagnostics.len())
    }
}

/// Checks programs against a statement catalogue.
pub struct Analyzer {
    builtins: HashMap<String, StatementSignature>,
    external: HashMap<String, External>,
    directory: Option<PathBuf>,
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
        let builtins: HashMap<String, StatementSignature> = signatures
            .into_iter()
            .map(|signature| (signature.normalized().to_owned(), signature.clone()))
            .collect();
        let external = EXTERNAL_STATEMENTS
            .iter()
            .filter_map(|(header, external)| {
                let normalized = StatementSignature::native(header)
                    .ok()?
                    .normalized()
                    .to_owned();
                builtins
                    .contains_key(&normalized)
                    .then_some((normalized, *external))
            })
            .collect();
        Self {
            builtins,
            external,
            directory: None,
        }
    }

    /// Read and check imported local modules, resolving relative source names
    /// against `directory` as a run in that directory would.
    pub fn with_modules(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    /// Findings for a script, in source order.
    pub fn check_program(&self, program: &Program) -> Vec<Finding> {
        self.report_program(program).findings
    }

    /// A script's findings and what the check could not establish.
    pub fn report_program(&self, program: &Program) -> Report {
        self.check(&[(program, &HashSet::new())])
    }

    /// Findings for a suite's setup, teardown, and every case, in source order.
    /// Cases see the variables their suite setup assigns and their row binding.
    pub fn check_suite(&self, suite: &Suite) -> Vec<Finding> {
        self.report_suite(suite).findings
    }

    /// A suite's findings and what the check could not establish.
    pub fn report_suite(&self, suite: &Suite) -> Report {
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

    fn check(&self, programs: &[(&Program, &HashSet<String>)]) -> Report {
        let mut report = Report::default();
        let mut modules = Modules::default();
        for (program, inputs) in programs {
            let mut checker = Checker {
                analyzer: self,
                report: &mut report,
                modules: &mut modules,
                reported: HashSet::new(),
                entry: true,
            };
            checker.program(program, inputs);
        }
        // Library and fixture statements appear in every case program.
        let mut seen = HashSet::new();
        report.findings.retain(|finding| {
            seen.insert((
                finding.rule,
                finding.span.source().name().to_owned(),
                finding.span.start(),
                finding.message.clone(),
            ))
        });
        let entry = programs
            .first()
            .map(|(program, _)| program.source.name().to_owned());
        report.findings.sort_by(|left, right| {
            let key = |finding: &Finding| {
                let name = finding.span.source().name();
                (
                    entry.as_deref() != Some(name),
                    name.to_owned(),
                    finding.span.start(),
                    finding.rule,
                )
            };
            key(left).cmp(&key(right))
        });
        report
    }
}

#[derive(Default)]
struct FrameNames<'a> {
    definitions: HashMap<&'a str, Vec<&'a Statement>>,
    /// Import statements by normalized alias, in source order.
    namespaces: HashMap<String, Vec<&'a Statement>>,
}

/// Exported statements of a module: normalized signature to display header.
type Exports = HashMap<String, String>;

#[derive(Default)]
struct Modules {
    checked: HashMap<PathBuf, Option<Rc<Exports>>>,
    loading: Vec<PathBuf>,
    /// The package project of `@` imports, found at the first.
    project: Option<Result<Rc<crate::core::packages::Project>, String>>,
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
            StatementKind::Import { namespace, .. } => names
                .namespaces
                .entry(normalize_sentence(&namespace.text))
                .or_default()
                .push(statement),
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
    /// Each alias's module exports; None when the module was not checked.
    exports: HashMap<String, Option<Rc<Exports>>>,
}

/// How a qualified call's alias resolves at the call.
enum Namespace<'f> {
    Imported(Option<&'f Rc<Exports>>),
    Later(&'f Statement),
    Missing,
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
            exports: HashMap::new(),
        }
    }

    fn chain(&self) -> impl Iterator<Item = &Frame<'a>> {
        std::iter::successors(Some(self), |frame| frame.parent)
    }

    fn binds(&self, variable: &str) -> bool {
        self.chain()
            .any(|frame| frame.variables.contains(variable) || frame.inputs.contains(variable))
    }

    /// The visible variable whose name is nearest to `variable`, when one is a
    /// likely misspelling of it.
    fn nearest_variable(&self, variable: &str) -> Option<&str> {
        let visible = self.chain().flat_map(|frame| {
            frame
                .variables
                .iter()
                .copied()
                .chain(frame.inputs.iter().map(String::as_str))
        });
        crate::core::suggest::nearest(variable, visible)
    }

    /// Imports register when they run: an earlier import in this frame, or any
    /// import in an enclosing one, serves the call.
    fn namespace(&self, namespace: &str, position: usize) -> Namespace<'_> {
        if let Some(imports) = self.names.namespaces.get(namespace) {
            if imports.iter().any(|import| import.span.start() < position) {
                return Namespace::Imported(self.exports.get(namespace).and_then(Option::as_ref));
            }
            if self
                .chain()
                .skip(1)
                .all(|outer| !outer.names.namespaces.contains_key(namespace))
            {
                return Namespace::Later(imports[0]);
            }
        }
        self.chain()
            .skip(1)
            .find(|outer| outer.names.namespaces.contains_key(namespace))
            .map_or(Namespace::Missing, |outer| {
                Namespace::Imported(outer.exports.get(namespace).and_then(Option::as_ref))
            })
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

struct Checker<'a, 'r> {
    analyzer: &'a Analyzer,
    report: &'r mut Report,
    modules: &'r mut Modules,
    /// Undefined variables are reported at their first read only.
    reported: HashSet<String>,
    /// Whether this is the checked file rather than an imported module; only
    /// its unassigned variables can be input variables.
    entry: bool,
}

impl Checker<'_, '_> {
    fn program(&mut self, program: &Program, inputs: &HashSet<String>) {
        let frame = self.frame(None, &program.statements, &[], inputs);
        self.duplicates(&frame, true);
        self.block(&program.statements, &frame);
    }

    /// A frame with its imports' modules checked and their exports recorded.
    fn frame<'p>(
        &mut self,
        parent: Option<&'p Frame<'p>>,
        statements: &'p [Statement],
        parameters: &'p [super::ast::Name],
        inputs: &'p HashSet<String>,
    ) -> Frame<'p> {
        let mut frame = Frame::new(parent, statements, parameters, inputs);
        let mut imports: Vec<_> = frame
            .names
            .namespaces
            .iter()
            .map(|(alias, imports)| (alias.clone(), imports[0]))
            .collect();
        imports.sort_by_key(|(_, import)| import.span.start());
        for (alias, import) in imports {
            let exports = self.module(import);
            frame.exports.insert(alias, exports);
        }
        frame
    }

    /// The file an `@name/file` import names from `importer`, and its
    /// package's directory, through the project found at the first such import.
    fn package_module(
        &mut self,
        parsed: Result<(&str, PathBuf), String>,
        importer: &str,
        directory: &Path,
    ) -> Result<(PathBuf, PathBuf), String> {
        let (name, file) = parsed?;
        let base = import_base(importer, directory);
        let base = super::paths::canonicalize(&base)
            .map_err(|error| format!("{}: {error}", base.display()))?;
        let project = self
            .modules
            .project
            .get_or_insert_with(|| {
                let root = crate::core::packages::Project::find(&base).ok_or_else(|| {
                    format!(
                        "`@` imports need a {} at or above {}",
                        crate::core::packages::MANIFEST,
                        base.display()
                    )
                })?;
                crate::core::packages::Project::load(&root, None).map(Rc::new)
            })
            .clone()?;
        let requested = project.resolve(&base, name, &file)?;
        let package = project
            .directory(name)
            .map(Path::to_owned)
            .unwrap_or_default();
        Ok((requested, package))
    }

    /// Read, parse, and check an imported module once, returning its exports.
    fn module(&mut self, import: &Statement) -> Option<Rc<Exports>> {
        let StatementKind::Import {
            path, path_span, ..
        } = import.kind()
        else {
            return None;
        };
        let directory = self.analyzer.directory.as_ref()?;
        // A Python, JavaScript, or WebAssembly module's statements are known
        // only when it loads, so calls through it are reported unchecked, not
        // as errors.
        if matches!(
            Path::new(path).extension().and_then(|value| value.to_str()),
            Some("py" | "js" | "mjs" | "cjs" | "wasm")
        ) {
            return None;
        }
        let (requested, package) = match crate::core::packages::package_path(path) {
            None => (
                module_path(path_span.source().name(), directory, path),
                None,
            ),
            Some(parsed) => match self.package_module(parsed, path_span.source().name(), directory)
            {
                Ok((requested, package)) => (Some(requested), Some(package)),
                Err(reason) => {
                    self.push(
                        Rule::ImportFailure,
                        path_span,
                        format!("Loading module failed: {reason}"),
                        "Name the package in botwork.toml and run `botwork --fetch`.",
                    );
                    return None;
                }
            },
        };
        let Some(requested) = requested.filter(|requested| {
            requested.extension().and_then(|value| value.to_str()) == Some("botwork")
        }) else {
            self.push(
                Rule::ImportFailure,
                path_span,
                format!("Loading module failed: `{path}` must name a local .botwork file"),
                "Import a local .botwork file by a path relative to the importing file.",
            );
            return None;
        };
        let canonical = match super::paths::canonicalize(&requested) {
            Ok(canonical) => canonical,
            Err(error) => {
                self.push(
                    Rule::ImportFailure,
                    path_span,
                    format!("Loading module failed: {}: {error}", requested.display()),
                    "Check the path; it is relative to the importing file's directory.",
                );
                return None;
            }
        };
        if let Some(package) = package.filter(|package| !canonical.starts_with(package)) {
            self.push(
                Rule::ImportFailure,
                path_span,
                format!(
                    "Loading module failed: `{path}` leads outside its package {}, to {}",
                    package.display(),
                    canonical.display()
                ),
                "Keep a package's files inside its directory.",
            );
            return None;
        }
        if let Some(start) = self
            .modules
            .loading
            .iter()
            .position(|loading| *loading == canonical)
        {
            let chain: Vec<String> = self.modules.loading[start..]
                .iter()
                .chain(std::iter::once(&canonical))
                .map(|path| path.display().to_string())
                .collect();
            self.push(
                Rule::ImportCycle,
                path_span,
                format!("Import cycle: {}", chain.join(" -> ")),
                "Break the cycle; move shared statements into a module that imports neither.",
            );
            return None;
        }
        if let Some(checked) = self.modules.checked.get(&canonical) {
            return checked.clone();
        }
        let text = fs::File::open(&canonical)
            .and_then(|file| {
                let mut bytes = Vec::new();
                file.take(DEFAULT_SOURCE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                Ok(bytes)
            })
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                if bytes.len() > DEFAULT_SOURCE_BYTES {
                    return Err(format!("larger than {DEFAULT_SOURCE_BYTES} bytes"));
                }
                String::from_utf8(bytes).map_err(|error| error.to_string())
            });
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                self.push(
                    Rule::ImportFailure,
                    path_span,
                    format!("Loading module failed: {}: {error}", canonical.display()),
                    "Check that the module is a readable UTF-8 file.",
                );
                self.modules.checked.insert(canonical, None);
                return None;
            }
        };
        let name = canonical.to_string_lossy().into_owned();
        let program = match Program::parse_detailed(&name, &text) {
            Ok(program) => program,
            Err(diagnostic) => {
                self.report.diagnostics.push(diagnostic);
                self.modules.checked.insert(canonical, None);
                return None;
            }
        };
        self.modules.loading.push(canonical.clone());
        let inputs = HashSet::new();
        let mut nested = Checker {
            analyzer: self.analyzer,
            report: &mut *self.report,
            modules: &mut *self.modules,
            reported: HashSet::new(),
            entry: false,
        };
        let frame = nested.frame(None, &program.statements, &[], &inputs);
        nested.duplicates(&frame, true);
        nested.block(&program.statements, &frame);
        // A module publishes its root definitions and, qualified, its imports'.
        let mut exports = Exports::new();
        for (signature, definitions) in &frame.names.definitions {
            if let StatementKind::Define(definition) = definitions[0].kind() {
                exports.insert(
                    (*signature).to_owned(),
                    definition.header.text().trim().to_owned(),
                );
            }
        }
        for (alias, imported) in &frame.exports {
            for (signature, header) in imported.iter().flat_map(|exports| exports.iter()) {
                exports.insert(
                    format!("{alias}::{signature}"),
                    format!("{alias}::{header}"),
                );
            }
        }
        self.modules.loading.pop();
        self.report.modules.insert(canonical.clone());
        let exports = Some(Rc::new(exports));
        self.modules.checked.insert(canonical, exports.clone());
        exports
    }

    fn push(&mut self, rule: Rule, span: &Span, message: String, help: impl Into<String>) {
        self.report.findings.push(Finding {
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
                if let Some(builtin) = self.analyzer.builtins.get(*signature) {
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
        let mut namespaces: Vec<_> = frame.names.namespaces.iter().collect();
        namespaces.sort_by_key(|(_, imports)| imports[0].span.start());
        for (alias, imports) in namespaces {
            for duplicate in &imports[1..] {
                if let StatementKind::Import { namespace, .. } = duplicate.kind() {
                    self.push(
                        Rule::DuplicateNamespace,
                        &namespace.span,
                        format!(
                            "Duplicate namespace `{alias}`; first imported at {}",
                            imports[0].span.location()
                        ),
                        "Import each module under its own alias.",
                    );
                }
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
                    && self.analyzer.builtins.contains_key(&call.signature)
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
                let child = self.frame(
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
                if !frame.binds(name) && self.entry {
                    self.report.inputs.insert(name.clone());
                }
                if !frame.binds(name) && self.reported.insert(name.clone()) {
                    let help = match frame.nearest_variable(name) {
                        Some(near) => format!("Did you mean `{near}`? Otherwise, assign it first, or supply it as an input variable with --var or --vars-file."),
                        None => "Assign it first, or supply it as an input variable with --var or --vars-file.".into(),
                    };
                    self.push(
                        Rule::UndefinedVariable,
                        &expression.span,
                        format!("`{name}` is never assigned in a scope that reaches this read"),
                        help,
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
        if let Some((namespace, exported)) = signature.split_once("::") {
            match frame.namespace(namespace, call.span.start()) {
                Namespace::Imported(Some(exports)) => {
                    if !exports.contains_key(exported) {
                        let words = |signature: &str| sentence_words(signature);
                        let suggestion = exports
                            .iter()
                            .filter(|(candidate, _)| words(candidate) == words(exported))
                            .map(|(_, header)| header)
                            .min();
                        let help = match suggestion {
                            Some(header) => format!("Did you mean `{namespace}::{header}`? The module exports statements by their words and parameter positions."),
                            None => format!("The module imported as `{namespace}` does not define this statement."),
                        };
                        self.push(
                            Rule::UndefinedStatement,
                            &call.span,
                            format!("Statement not defined: {}", call.span.text().trim()),
                            help,
                        );
                    }
                }
                Namespace::Imported(None) => self.report.unchecked_imports += 1,
                Namespace::Later(import) => self.push(
                    Rule::StatementBeforeDefinition,
                    &call.span,
                    format!(
                        "`{}` is called before its import at {} runs",
                        call.span.text().trim(),
                        import.span.location()
                    ),
                    "Imports register when they run; move the import above this call.",
                ),
                Namespace::Missing => self.push(
                    Rule::UndefinedStatement,
                    &call.span,
                    format!("No module is imported as `{namespace}`"),
                    format!("Import a module with `Import |\"file.botwork\"| As |{namespace}|` before calling it."),
                ),
            }
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
        if let Some(builtin) = self.analyzer.builtins.get(signature) {
            self.arguments(call, builtin);
            if let Some(external) = self.analyzer.external.get(signature) {
                *self.report.external.entry(*external).or_default() += 1;
            }
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
        let wanted = sentence_words(signature);
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
            .chain(self.analyzer.builtins.iter().map(|(candidate, builtin)| {
                (
                    candidate.as_str(),
                    builtin.header().text().trim().to_owned(),
                )
            }))
            .filter(|(candidate, _)| sentence_words(candidate) == wanted)
            .map(|(candidate, header)| (candidate.to_owned(), header))
            .collect();
        candidates.pop_first().map(|(_, header)| header)
    }
}

/// Where an import's module file is, as the runtime resolves it: relative to the
/// importing file, whose relative name is relative to `directory`. None when
/// `path` does not name a local `.botwork` file.
pub(crate) fn module_path(importer: &str, directory: &Path, path: &str) -> Option<PathBuf> {
    if path.contains("://")
        || Path::new(path).extension().and_then(|value| value.to_str()) != Some("botwork")
    {
        return None;
    }
    let base = import_base(importer, directory);
    match crate::core::packages::import_file(&base, path) {
        Some(file) => file.ok(),
        None => Some(base.join(path)),
    }
}

/// The directory an import from the source `importer` resolves against.
fn import_base(importer: &str, directory: &Path) -> PathBuf {
    let importer = Path::new(importer);
    if importer.is_absolute() {
        importer.parent().unwrap_or(Path::new("/")).to_owned()
    } else {
        directory.join(importer.parent().unwrap_or(Path::new("")))
    }
}

/// A normalized signature's words without its parameter positions.
fn sentence_words(signature: &str) -> String {
    signature
        .replace("|param|", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
