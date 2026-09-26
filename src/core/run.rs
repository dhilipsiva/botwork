//! Fresh, synchronous runs with local configuration and structured outcomes.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use super::{
    ast::Program,
    ast_limits::AstLimits,
    diagnostic::{Diagnostic, DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::{BWErr, Literal, LiteralResult},
    operation::OperationControl,
    signature::StatementSignature,
    syntax_limits::{SyntaxLimits, DEFAULT_SOURCE_BYTES},
    value_limits::{Owned, ValueLimits},
};

mod import_limits;
pub(crate) use import_limits::ImportResource;
pub use import_limits::{ImportLimits, MAX_MODULE_CHAIN_DEPTH};
mod retained_values;
pub use retained_values::RetainedValueLimits;
pub(crate) use retained_values::{StoredValue, ValueReservation};

pub const DEFAULT_STEPS: u64 = 1_000_000;
pub const MAX_EVALUATION_DEPTH: usize = 96;
pub const MAX_IMPORT_DEPTH: usize = 16;
pub const MAX_PARSER_CALLER_DEPTH: usize = 16;

/// Run budgets. Temporary allocations and hard native termination
/// have separate contracts; these limits do not make execution a sandbox.
#[derive(Clone, Debug)]
pub struct RunLimits {
    pub source_bytes: usize,
    pub steps: u64,
    pub call_depth: usize,
    pub evaluation_depth: usize,
    pub import_depth: usize,
    pub syntax: SyntaxLimits,
    pub ast: AstLimits,
    pub imports: ImportLimits,
    pub values: ValueLimits,
    pub retained_values: RetainedValueLimits,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            source_bytes: DEFAULT_SOURCE_BYTES,
            steps: DEFAULT_STEPS,
            call_depth: 32,
            evaluation_depth: MAX_EVALUATION_DEPTH,
            import_depth: MAX_IMPORT_DEPTH,
            syntax: SyntaxLimits::default(),
            ast: AstLimits::default(),
            imports: ImportLimits::default(),
            values: ValueLimits::default(),
            retained_values: RetainedValueLimits::default(),
        }
    }
}

impl RunLimits {
    pub(crate) fn validate(&self) -> DiagnosticResult<()> {
        self.ast.validate()?;
        self.values.validate()?;
        if self.imports.dependency_depth > MAX_MODULE_CHAIN_DEPTH {
            return Err(BWErr::RunConfiguration(format!(
                "Module dependency depth cannot exceed {MAX_MODULE_CHAIN_DEPTH}"
            ))
            .into());
        }
        if self.import_depth > MAX_IMPORT_DEPTH {
            return Err(BWErr::RunConfiguration(format!(
                "Import initialization depth cannot exceed {MAX_IMPORT_DEPTH}"
            ))
            .into());
        }
        if self.evaluation_depth > MAX_EVALUATION_DEPTH {
            return Err(BWErr::RunConfiguration(format!(
                "Evaluation depth cannot exceed {MAX_EVALUATION_DEPTH}"
            ))
            .into());
        }
        super::syntax_limits::check("", self.source_bytes, &self.syntax, false)
            .map_err(|violation| Diagnostic::new(violation.error))
    }
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub variables: BTreeMap<String, Literal>,
    /// Relative paths resolve once against the host cwd; None captures that cwd.
    pub working_directory: Option<PathBuf>,
    pub inherit_environment: bool,
    /// Some replaces/adds an entry; None removes it from this run's snapshot.
    pub environment: BTreeMap<OsString, Option<OsString>>,
    pub control: OperationControl,
    pub timeout: Option<Duration>,
    pub limits: RunLimits,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            variables: BTreeMap::new(),
            working_directory: None,
            inherit_environment: true,
            environment: BTreeMap::new(),
            control: OperationControl::default(),
            timeout: None,
            limits: RunLimits::default(),
        }
    }
}

/// An immutable snapshot. Native operations must use these values explicitly.
#[derive(Clone, Debug)]
pub struct RunEnvironment {
    directory: PathBuf,
    variables: BTreeMap<OsString, OsString>,
    control: OperationControl,
}

impl RunEnvironment {
    pub fn working_directory(&self) -> &Path {
        &self.directory
    }
    pub fn variables(&self) -> &BTreeMap<OsString, OsString> {
        &self.variables
    }
    pub fn get(&self, name: impl AsRef<OsStr>) -> Option<&OsStr> {
        self.variables.get(name.as_ref()).map(OsString::as_os_str)
    }
    pub fn control(&self) -> &OperationControl {
        &self.control
    }

    fn prepare(options: &RunOptions, start: tokio::time::Instant) -> DiagnosticResult<Self> {
        options.control.checkpoint()?;
        let failure = |reason: String| Diagnostic::new(BWErr::RunConfiguration(reason));
        let deadline = options
            .timeout
            .map(|timeout| {
                start
                    .checked_add(timeout)
                    .ok_or_else(|| failure("Timeout exceeds the monotonic clock range".into()))
            })
            .transpose()?;
        let control = options.control.child(deadline);
        control.checkpoint()?;
        let directory = options
            .working_directory
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|error| failure(error.to_string()))?;
        let directory = fs::canonicalize(&directory)
            .map_err(|error| failure(format!("{}: {error}", directory.display())))?;
        if !directory.is_dir() {
            return Err(failure("Working directory must be a directory".into()));
        }
        let mut variables: BTreeMap<OsString, OsString> = if options.inherit_environment {
            std::env::vars_os().collect()
        } else {
            BTreeMap::new()
        };
        for (name, value) in &options.environment {
            if name.is_empty()
                || name.as_encoded_bytes().contains(&b'=')
                || name.as_encoded_bytes().contains(&0)
            {
                return Err(failure(
                    "Environment names must be nonempty and contain neither '=' nor NUL".into(),
                ));
            }
            if value
                .as_ref()
                .is_some_and(|value| value.as_encoded_bytes().contains(&0))
            {
                return Err(failure("Environment values must not contain NUL".into()));
            }
            match value {
                Some(value) => {
                    variables.insert(name.clone(), value.clone());
                }
                None => {
                    variables.remove(name);
                }
            }
        }
        control.checkpoint()?;
        Ok(Self {
            directory,
            variables,
            control,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    LimitExceeded,
}

#[derive(Debug)]
pub struct RunResult {
    pub result: DiagnosticResult<Literal>,
    /// Completed root assignments, including inputs; invocation locals never escape.
    pub variables: BTreeMap<String, Literal>,
    pub steps: u64,
    pub elapsed: Duration,
}

impl RunResult {
    pub fn outcome(&self) -> RunOutcome {
        match &self.result {
            Ok(_) => RunOutcome::Succeeded,
            Err(error) => match error.code() {
                DiagnosticCode::Cancelled => RunOutcome::Cancelled,
                DiagnosticCode::Timeout => RunOutcome::TimedOut,
                DiagnosticCode::ResourceLimit => RunOutcome::LimitExceeded,
                _ => RunOutcome::Failed,
            },
        }
    }
}

/// Reusable native registrations; every execution gets fresh DSL/module state.
/// Captured callback state is intentionally shared across engine clones/runs.
#[derive(Clone)]
pub struct Engine {
    template: Context,
}

impl Default for Engine {
    fn default() -> Self {
        let mut template = Context::default();
        template.init_statements();
        Self { template }
    }
}

impl Engine {
    pub fn register_native(
        &mut self,
        header: &str,
        callback: impl Fn(&[Literal], &RunEnvironment) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        self.register_native_with_signature(StatementSignature::native(header)?, callback)
    }

    pub fn register_native_with_signature(
        &mut self,
        signature: StatementSignature,
        callback: impl Fn(&[Literal], &RunEnvironment) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        self.template.register_run_native(signature, callback)
    }

    pub fn run_source(&self, name: &str, source: &str, options: RunOptions) -> RunResult {
        self.run(options, |context| {
            context.check_source_size(source.len())?;
            let program = context.parse_source(name, source)?;
            evaluate_program_detailed(&program, context)
        })
    }

    pub fn run_program(&self, program: &Program, options: RunOptions) -> RunResult {
        self.run(options, |context| {
            context.check_source_size(program.source.text().len())?;
            context.check_syntax(program.source.name(), program.source.text())?;
            evaluate_program_detailed(program, context)
        })
    }

    pub fn run_file(&self, path: impl AsRef<Path>, options: RunOptions) -> RunResult {
        self.run(options, |context| {
            let path = context
                .environment
                .as_ref()
                .expect("configured run")
                .directory
                .join(path);
            let name = path
                .to_str()
                .ok_or_else(|| BWErr::SourceRead("Source paths must be valid UTF-8".into()))?;
            let source = context.read_source(&path).map_err(|error| match error {
                SourceFailure::Io(error) => {
                    Diagnostic::new(BWErr::SourceRead(format!("{name}: {error}")))
                }
                SourceFailure::Diagnostic(error) => error,
            })?;
            let program = context.parse_source(name, &source)?;
            evaluate_program_detailed(&program, context)
        })
    }

    fn run(
        &self,
        mut options: RunOptions,
        execute: impl FnOnce(&mut Context) -> DiagnosticResult<Literal>,
    ) -> RunResult {
        let start = Instant::now();
        let control_start = tokio::time::Instant::now();
        let mut context = self.template.clone();
        let variables = Owned::new(std::mem::take(&mut options.variables));
        let result = (|| {
            options.control.checkpoint()?;
            options.limits.validate()?;
            let environment = Arc::new(RunEnvironment::prepare(&options, control_start)?);
            context.working_directory = Ok(environment.directory.clone());
            context.budget = Some(RunBudget::new(options.limits, environment.control.clone()));
            context.environment = Some(environment);
            context.set_input_variables(variables.into_inner())?;
            context.checkpoint()?;
            let result = execute(&mut context);
            context.after_operation(result)
        })();
        RunResult {
            result,
            variables: context.root_variables(),
            steps: context.budget.as_ref().map_or(0, |budget| budget.used()),
            elapsed: start.elapsed(),
        }
    }
}

struct BudgetState {
    limits: RunLimits,
    control: OperationControl,
    used: AtomicU64,
    active: AtomicUsize,
    imports: Mutex<[usize; 5]>,
    retained_values: Arc<retained_values::RetainedValues>,
    stopped: Mutex<Option<BWErr>>,
}

pub(crate) struct RunBudget(Arc<BudgetState>);

// Context clones copy work counters but share live allocation accounting.
// Modules explicitly share all budgets.
impl Clone for RunBudget {
    fn clone(&self) -> Self {
        Self(Arc::new(BudgetState {
            limits: self.0.limits.clone(),
            control: self.0.control.clone(),
            used: AtomicU64::new(self.used()),
            active: AtomicUsize::new(0),
            imports: Mutex::new(*self.0.imports.lock().unwrap_or_else(|e| e.into_inner())),
            retained_values: Arc::clone(&self.0.retained_values),
            stopped: Mutex::new(
                self.0
                    .stopped
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone(),
            ),
        }))
    }
}

impl RunBudget {
    pub(crate) fn new(limits: RunLimits, control: OperationControl) -> Self {
        Self(Arc::new(BudgetState {
            retained_values: Arc::new(retained_values::RetainedValues::new(
                limits.retained_values.clone(),
            )),
            limits,
            control,
            used: AtomicU64::new(0),
            active: AtomicUsize::new(0),
            imports: Mutex::new([0; 5]),
            stopped: Mutex::new(None),
        }))
    }
    pub(crate) fn shared(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
    pub(crate) fn limits(&self) -> &RunLimits {
        &self.0.limits
    }
    fn used(&self) -> u64 {
        self.0.used.load(Ordering::Relaxed)
    }
    fn stop(&self, error: BWErr) -> Diagnostic {
        let mut stopped = self.0.stopped.lock().unwrap_or_else(|e| e.into_inner());
        Diagnostic::new(stopped.get_or_insert(error).clone())
    }
    pub(crate) fn limit(&self, resource: &'static str, limit: u64) -> Diagnostic {
        self.stop(BWErr::ResourceLimit { resource, limit })
    }
    pub(crate) fn checkpoint(&self) -> DiagnosticResult<()> {
        if let Some(error) = self
            .0
            .stopped
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Err(Diagnostic::new(error));
        }
        self.0
            .control
            .checkpoint()
            .map_err(|error| self.stop(error.into_error()))
    }
    pub(crate) fn tick(&self) -> DiagnosticResult<()> {
        self.checkpoint()?;
        self.0
            .used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < self.0.limits.steps).then(|| used + 1)
            })
            .map(|_| ())
            .map_err(|_| self.limit("evaluation steps", self.0.limits.steps))
    }

    pub(crate) fn enter(&self) -> DiagnosticResult<EvaluationGuard> {
        self.checkpoint()?;
        self.0
            .active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                (active < self.0.limits.evaluation_depth).then(|| active + 1)
            })
            .map_err(|_| self.limit("evaluation depth", self.0.limits.evaluation_depth as u64))?;
        Ok(EvaluationGuard(self.shared()))
    }

    pub(crate) fn check_parser_entry(&self) -> DiagnosticResult<()> {
        self.checkpoint()?;
        if self.0.active.load(Ordering::Relaxed) > MAX_PARSER_CALLER_DEPTH {
            return Err(self.limit("parser caller depth", MAX_PARSER_CALLER_DEPTH as u64));
        }
        Ok(())
    }
}

pub(crate) struct EvaluationGuard(RunBudget);

impl Drop for EvaluationGuard {
    fn drop(&mut self) {
        self.0 .0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) enum SourceFailure {
    Io(std::io::Error),
    Diagnostic(Diagnostic),
}

pub(crate) fn read_source(path: &Path, max: Option<usize>) -> Result<Vec<u8>, SourceFailure> {
    let file = fs::File::open(path).map_err(SourceFailure::Io)?;
    let mut source = Vec::new();
    file.take(max.map_or(u64::MAX, |max| (max as u64).saturating_add(1)))
        .read_to_end(&mut source)
        .map_err(SourceFailure::Io)?;
    Ok(source)
}
