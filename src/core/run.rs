//! Fresh runs with local configuration and structured outcomes.

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
    acceptance::CaseStatus,
    ast::Program,
    ast_limits::AstLimits,
    diagnostic::{
        Diagnostic, DiagnosticCode, DiagnosticLimits, DiagnosticResult, DiagnosticValueLimits,
    },
    eval::{evaluate_program_runtime, Context, EvaluationResult},
    grammar::{BWErr, Literal, LiteralResult},
    operation::OperationControl,
    signature::StatementSignature,
    stack,
    syntax_limits::{SyntaxLimits, DEFAULT_SOURCE_BYTES},
    value_limits::{Owned, ValueLimits},
};

#[cfg(test)]
mod tests;

mod asynchronous;
mod attempt;
mod cleanup;
pub use cleanup::CleanupLimits;
pub(crate) mod blocking_io;
mod import_limits;
mod output_limits;
pub use import_limits::ImportLimits;
pub(crate) use import_limits::ImportResource;
#[doc(hidden)]
pub use import_limits::MAX_MODULE_CHAIN_DEPTH;
pub use output_limits::OutputLimits;
#[doc(hidden)]
pub use output_limits::{DEFAULT_OUTPUT_BYTES, DEFAULT_OUTPUT_RECORD_BYTES};
mod retained_values;
pub use retained_values::RetainedValueLimits;
pub(crate) use retained_values::{StoredValue, ValueReservation};
mod retained_definitions;
pub(crate) use retained_definitions::DefinitionReservation;
pub use retained_definitions::RetainedDefinitionLimits;
mod retained_names;
pub(crate) use retained_names::RetainedName;
pub use retained_names::RetainedNameLimits;
mod retained_registry;
pub use retained_registry::RetainedRegistryLimits;
pub(crate) use retained_registry::{RegistryPlan, RegistryReservation};
mod snapshot_limits;
pub use snapshot_limits::SnapshotLimits;
pub(crate) use snapshot_limits::SnapshotSize;
mod result_limits;
pub use result_limits::ResultLimits;
mod retained_diagnostics;
pub use retained_diagnostics::RetainedDiagnosticLimits;
pub(crate) use retained_diagnostics::{RuntimeDiagnostic, StoredCallFrame, StoredDiagnostic};
mod temporary_values;
pub use temporary_values::TemporaryLimits;
mod wasm_limits;
pub(crate) use temporary_values::{TemporaryReservation, TemporaryValue};
pub use wasm_limits::{WasmLimits, MAX_WASM_MEMORY_BYTES};

#[doc(hidden)]
pub const DEFAULT_STEPS: u64 = 1_000_000;
pub const MAX_EVALUATION_DEPTH: usize = 96;
#[doc(hidden)]
pub const MAX_IMPORT_DEPTH: usize = 16;
pub(crate) const MAX_PARSER_CALLER_DEPTH: usize = 16;

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
    pub retained_definitions: RetainedDefinitionLimits,
    pub retained_names: RetainedNameLimits,
    pub retained_registry: RetainedRegistryLimits,
    pub snapshots: SnapshotLimits,
    pub results: ResultLimits,
    pub temporaries: TemporaryLimits,
    pub diagnostic_values: DiagnosticValueLimits,
    pub diagnostics: DiagnosticLimits,
    pub retained_diagnostics: RetainedDiagnosticLimits,
    pub output: OutputLimits,
    pub cleanup: CleanupLimits,
    pub wasm: WasmLimits,
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
            retained_definitions: RetainedDefinitionLimits::default(),
            retained_names: RetainedNameLimits::default(),
            retained_registry: RetainedRegistryLimits::default(),
            snapshots: SnapshotLimits::default(),
            results: ResultLimits::default(),
            temporaries: TemporaryLimits::default(),
            diagnostic_values: DiagnosticValueLimits::default(),
            diagnostics: DiagnosticLimits::default(),
            retained_diagnostics: RetainedDiagnosticLimits::default(),
            output: OutputLimits::default(),
            cleanup: CleanupLimits::default(),
            wasm: WasmLimits::default(),
        }
    }
}

impl RunLimits {
    pub(crate) fn validate(&self) -> DiagnosticResult<()> {
        self.ast.validate()?;
        self.values.validate()?;
        self.diagnostic_values.values.validate()?;
        self.diagnostics.validate()?;
        self.cleanup.validate()?;
        self.wasm.validate()?;
        if self.imports.dependency_depth > MAX_MODULE_CHAIN_DEPTH {
            return Err(Diagnostic::formatted(
                BWErr::RunConfiguration,
                format_args!("Module dependency depth cannot exceed {MAX_MODULE_CHAIN_DEPTH}"),
            ));
        }
        if self.import_depth > MAX_IMPORT_DEPTH {
            return Err(Diagnostic::formatted(
                BWErr::RunConfiguration,
                format_args!("Import initialization depth cannot exceed {MAX_IMPORT_DEPTH}"),
            ));
        }
        if self.evaluation_depth > MAX_EVALUATION_DEPTH {
            return Err(Diagnostic::formatted(
                BWErr::RunConfiguration,
                format_args!("Evaluation depth cannot exceed {MAX_EVALUATION_DEPTH}"),
            ));
        }
        super::syntax_limits::check("", self.source_bytes, &self.syntax, false)
            .map(|_| ())
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
    /// Some records this run's statements, logs, error, and timing in `RunResult::record`.
    pub record: Option<crate::core::report::RecordOptions>,
    /// Texts masked from this run's output and record; see [`crate::core::secret`].
    pub secrets: crate::core::secret::Secrets,
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
            record: None,
            secrets: crate::core::secret::Secrets::default(),
        }
    }
}

/// An immutable snapshot. Native operations must use these values explicitly.
#[derive(Clone, Debug)]
pub struct RunEnvironment {
    directory: Arc<PathBuf>,
    variables: Arc<BTreeMap<OsString, OsString>>,
    control: OperationControl,
    /// Shared by every worker, module, cleanup, and attempt context of a recorded run.
    pub(crate) recorder: Option<crate::core::report::Recorder>,
    /// Masked from every output of the run and its workers.
    pub(crate) secrets: crate::core::secret::Secrets,
    /// Parsed modules shared with other runs; module state stays per run.
    pub(crate) compiled: Option<crate::core::eval::CompiledModules>,
}

impl Context {
    /// Mask `secrets` from this context's output. The context needs a run
    /// environment, as from [`Context::with_host_environment`].
    pub fn set_secrets(&mut self, secrets: &crate::core::secret::Secrets) -> DiagnosticResult<()> {
        let environment = self.environment.as_ref().ok_or_else(|| {
            Diagnostic::new(BWErr::RunConfiguration(
                "Secrets require a run environment".into(),
            ))
        })?;
        let mut environment = RunEnvironment::clone(environment);
        environment.secrets = secrets.clone();
        self.environment = Some(Arc::new(environment));
        Ok(())
    }

    /// Share parsed imported modules with every run given the same cache. The
    /// context needs a run environment, as from [`Context::with_host_environment`].
    pub fn set_compiled_modules(
        &mut self,
        modules: &crate::core::eval::CompiledModules,
    ) -> DiagnosticResult<()> {
        let environment = self.environment.as_ref().ok_or_else(|| {
            Diagnostic::new(BWErr::RunConfiguration(
                "Compiled modules require a run environment".into(),
            ))
        })?;
        let mut environment = RunEnvironment::clone(environment);
        environment.compiled = Some(modules.clone());
        self.environment = Some(Arc::new(environment));
        Ok(())
    }

    /// The texts masked from this context's output.
    pub(crate) fn secrets(&self) -> Option<&crate::core::secret::Secrets> {
        self.environment
            .as_ref()
            .map(|environment| &environment.secrets)
    }

    /// Record this context's statements and logs into `recording`. The context
    /// needs a run environment, as from [`Context::with_host_environment`].
    pub fn attach_recording(
        &mut self,
        recording: &crate::core::report::Recording,
    ) -> DiagnosticResult<()> {
        let environment = self.environment.as_ref().ok_or_else(|| {
            Diagnostic::new(BWErr::RunConfiguration(
                "Recording requires a run environment".into(),
            ))
        })?;
        let mut environment = RunEnvironment::clone(environment);
        environment.recorder = Some(recording.recorder().clone());
        self.environment = Some(Arc::new(environment));
        Ok(())
    }

    /// A fresh, controlled context with a canonical current directory and an
    /// immutable snapshot of the host environment. Capture performs blocking OS
    /// work; async hosts should call this during worker-based preparation.
    /// Unlike Engine, this does not initialize statements or copy a template.
    pub fn with_host_environment(
        limits: RunLimits,
        control: OperationControl,
    ) -> DiagnosticResult<Self> {
        control.checkpoint()?;
        limits.validate()?;
        let environment = Arc::new(RunEnvironment::prepare_with_control(
            &RunOptions::default(),
            control.clone(),
        )?);
        let mut context =
            Self::with_directory_snapshot(Ok(environment.working_directory().to_owned()));
        context.budget = Some(RunBudget::new(limits, control));
        context.environment = Some(environment);
        Ok(context)
    }
}

/// Whether environment names match without regard to ASCII case: on Windows,
/// as the operating system matches them.
const NAMES_IGNORE_CASE: bool = cfg!(windows);

/// Whether two environment names name the same variable on this platform.
pub(crate) fn same_environment_name(left: &OsStr, right: &OsStr) -> bool {
    if NAMES_IGNORE_CASE {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// The stored name that `name` names among `variables`.
fn environment_key<'a>(
    variables: &'a BTreeMap<OsString, OsString>,
    name: &OsStr,
    ignore_case: bool,
) -> Option<&'a OsString> {
    if ignore_case {
        variables.keys().find(|key| key.eq_ignore_ascii_case(name))
    } else {
        variables.get_key_value(name).map(|(key, _)| key)
    }
}

/// Set or remove `name`, replacing the variable it names whatever that
/// variable's case where names ignore case.
fn overlay(
    variables: &mut BTreeMap<OsString, OsString>,
    name: &OsStr,
    value: Option<&OsString>,
    ignore_case: bool,
) {
    if let Some(key) = environment_key(variables, name, ignore_case).cloned() {
        variables.remove(&key);
    }
    if let Some(value) = value {
        variables.insert(name.to_owned(), value.clone());
    }
}

impl RunEnvironment {
    pub(crate) fn with_control(&self, control: OperationControl) -> Self {
        Self {
            directory: Arc::clone(&self.directory),
            variables: Arc::clone(&self.variables),
            control,
            recorder: self.recorder.clone(),
            secrets: self.secrets.clone(),
            compiled: self.compiled.clone(),
        }
    }
    pub fn working_directory(&self) -> &Path {
        &self.directory
    }
    pub fn variables(&self) -> &BTreeMap<OsString, OsString> {
        &self.variables
    }
    /// The variable `name` names; on Windows its case does not matter.
    pub fn get(&self, name: impl AsRef<OsStr>) -> Option<&OsStr> {
        environment_key(&self.variables, name.as_ref(), NAMES_IGNORE_CASE)
            .map(|key| self.variables[key].as_os_str())
    }
    pub fn control(&self) -> &OperationControl {
        &self.control
    }

    fn configuration_error(reason: std::fmt::Arguments<'_>) -> Diagnostic {
        Diagnostic::formatted(BWErr::RunConfiguration, reason)
    }

    fn control_at(
        options: &RunOptions,
        start: tokio::time::Instant,
    ) -> DiagnosticResult<OperationControl> {
        options.control.checkpoint()?;
        let failure = Self::configuration_error;
        let deadline = options
            .timeout
            .map(|timeout| {
                start.checked_add(timeout).ok_or_else(|| {
                    failure(format_args!("Timeout exceeds the monotonic clock range"))
                })
            })
            .transpose()?;
        let control = options.control.child(deadline);
        control.checkpoint()?;
        Ok(control)
    }

    fn prepare(options: &RunOptions, start: tokio::time::Instant) -> DiagnosticResult<Self> {
        Self::prepare_with_control(options, Self::control_at(options, start)?)
    }

    fn prepare_with_control(
        options: &RunOptions,
        control: OperationControl,
    ) -> DiagnosticResult<Self> {
        control.checkpoint()?;
        let failure = Self::configuration_error;
        let directory = options
            .working_directory
            .clone()
            .map(Ok)
            .unwrap_or_else(std::env::current_dir)
            .map_err(|error| failure(format_args!("{error}")))?;
        let directory = super::paths::canonicalize(&directory)
            .map_err(|error| failure(format_args!("{}: {error}", directory.display())))?;
        control.checkpoint()?;
        if !directory.is_dir() {
            return Err(failure(format_args!(
                "Working directory must be a directory"
            )));
        }
        control.checkpoint()?;
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
                return Err(failure(format_args!(
                    "Environment names must be nonempty and contain neither '=' nor NUL"
                )));
            }
            if value
                .as_ref()
                .is_some_and(|value| value.as_encoded_bytes().contains(&0))
            {
                return Err(failure(format_args!(
                    "Environment values must not contain NUL"
                )));
            }
            overlay(&mut variables, name, value.as_ref(), NAMES_IGNORE_CASE);
        }
        control.checkpoint()?;
        Ok(Self {
            directory: Arc::new(directory),
            variables: Arc::new(variables),
            control,
            recorder: None,
            secrets: options.secrets.clone(),
            compiled: None,
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
#[non_exhaustive]
pub struct RunResult {
    pub result: DiagnosticResult<Literal>,
    /// Completed root assignments, including inputs, unless snapshot_error is Some.
    /// Invocation locals never escape.
    pub variables: BTreeMap<String, Literal>,
    /// Some means aggregate export admission failed and variables were omitted.
    /// An earlier execution error remains primary; otherwise result is this error.
    pub snapshot_error: Option<Diagnostic>,
    pub steps: u64,
    pub elapsed: Duration,
    /// Present when `RunOptions::record` requested recording.
    pub record: Option<crate::core::report::RunRecord>,
}

impl RunResult {
    pub fn outcome(&self) -> RunOutcome {
        match &self.result {
            Ok(_) => RunOutcome::Succeeded,
            Err(error) => match CaseStatus::from_diagnostic_code(error.code()) {
                CaseStatus::Cancelled => RunOutcome::Cancelled,
                CaseStatus::TimedOut => RunOutcome::TimedOut,
                CaseStatus::LimitExceeded => RunOutcome::LimitExceeded,
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
    /// Parsed imported modules shared by this engine's runs.
    compiled: crate::core::eval::CompiledModules,
}

impl Default for Engine {
    fn default() -> Self {
        let mut template = Context::default();
        template.init_statements();
        Self {
            template,
            compiled: crate::core::eval::CompiledModules::default(),
        }
    }
}

impl Engine {
    /// Register an operation for run_source_async, run_program_async, or run_file_async.
    /// Synchronous runs reject an engine containing operations before executing statements.
    pub fn register_operation(
        &mut self,
        operation: super::operation::NativeOperation,
    ) -> DiagnosticResult<()> {
        self.template.register_operation(operation)
    }
    /// Configure reusable native registration storage. RunOptions still supplies
    /// each execution's independent registry budgets, checked before input/effects.
    pub fn with_registry_limits(retained_registry: RetainedRegistryLimits) -> Self {
        let mut template = Context::with_limits(RunLimits {
            retained_registry,
            ..RunLimits::default()
        })
        .expect("registry budgets have no fixed ceilings");
        template.init_statements();
        Self {
            template,
            compiled: crate::core::eval::CompiledModules::default(),
        }
    }

    /// The cache of parsed imported modules this engine's runs share.
    pub fn compiled_modules(&self) -> &crate::core::eval::CompiledModules {
        &self.compiled
    }

    /// Share `modules` with other engines or hosts instead of this engine's own cache.
    pub fn share_compiled_modules(&mut self, modules: &crate::core::eval::CompiledModules) {
        self.compiled = modules.clone();
    }

    /// Callbacks use workers in async runs and the calling thread in synchronous runs.
    pub fn register_native(
        &mut self,
        header: &str,
        callback: impl Fn(&[Literal], &RunEnvironment) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        self.register_native_with_signature(StatementSignature::native(header)?, callback)
    }

    /// Register a typed callback with the same worker/thread contract as `register_native`.
    pub fn register_native_with_signature(
        &mut self,
        signature: StatementSignature,
        callback: impl Fn(&[Literal], &RunEnvironment) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        self.template.register_run_native(signature, callback)
    }

    pub fn run_source(&self, name: &str, source: &str, options: RunOptions) -> RunResult {
        self.run(options, name, |context| {
            context.check_source_size(source.len())?;
            let program = context.parse_source(name, source)?;
            evaluate_program_runtime(&program, context)
        })
    }

    pub fn run_program(&self, program: &Program, options: RunOptions) -> RunResult {
        self.run(options, program.source.name(), |context| {
            context.check_source_size(program.source.text().len())?;
            context.check_syntax(program.source.name(), program.source.text())?;
            evaluate_program_runtime(program, context)
        })
    }

    pub fn run_file(&self, path: impl AsRef<Path>, options: RunOptions) -> RunResult {
        let name = path.as_ref().display().to_string();
        self.run(options, &name, |context| {
            let path = context
                .environment
                .as_ref()
                .expect("configured run")
                .directory
                .join(path);
            let name = path.to_str().ok_or_else(|| {
                context.formatted_error(
                    BWErr::SourceRead,
                    format_args!("Source paths must be valid UTF-8"),
                    None,
                    false,
                )
            })?;
            let source = context.read_source(&path).map_err(|error| match error {
                SourceFailure::Io(error) => context.formatted_error(
                    BWErr::SourceRead,
                    format_args!("{name}: {error}"),
                    None,
                    false,
                ),
                SourceFailure::Diagnostic(error) => error.into(),
            })?;
            let program = context.parse_source(name, &source)?;
            evaluate_program_runtime(&program, context)
        })
    }

    fn run(
        &self,
        options: RunOptions,
        name: &str,
        execute: impl FnOnce(&mut Context) -> EvaluationResult<Literal>,
    ) -> RunResult {
        let (mut active, prepared) =
            self.prepare_run(asynchronous::PendingRun::new(options, name), false);
        let result = prepared.and_then(|()| execute(&mut active.context));
        active.finish(result)
    }
}

struct BudgetState {
    limits: RunLimits,
    control: OperationControl,
    // Shared by polling attempts; clones and cleanup allowances own fresh counters.
    used: Arc<AtomicU64>,
    active: AtomicUsize,
    output: Arc<AtomicUsize>,
    imports: Arc<Mutex<[usize; 5]>>,
    snapshots: Arc<Mutex<[usize; 2]>>,
    cleanup_steps: Arc<AtomicU64>,
    cleaning: bool,
    retained_values: Arc<retained_values::RetainedValues>,
    retained_definitions: Arc<retained_definitions::RetainedDefinitions>,
    retained_names: Arc<retained_names::RetainedNames>,
    retained_registry: Arc<retained_registry::RetainedRegistry>,
    temporary_values: Arc<temporary_values::TemporaryValues>,
    retained_diagnostics: Arc<retained_diagnostics::RetainedDiagnostics>,
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
            used: Arc::new(AtomicU64::new(self.0.used.load(Ordering::Relaxed))),
            active: AtomicUsize::new(0),
            output: Arc::new(AtomicUsize::new(self.0.output.load(Ordering::Relaxed))),
            imports: Arc::new(Mutex::new(
                *self.0.imports.lock().unwrap_or_else(|e| e.into_inner()),
            )),
            snapshots: Arc::new(Mutex::new(
                *self.0.snapshots.lock().unwrap_or_else(|e| e.into_inner()),
            )),
            cleanup_steps: Arc::new(AtomicU64::new(self.0.cleanup_steps.load(Ordering::Relaxed))),
            cleaning: self.0.cleaning,
            retained_values: Arc::clone(&self.0.retained_values),
            retained_definitions: Arc::clone(&self.0.retained_definitions),
            retained_names: Arc::clone(&self.0.retained_names),
            retained_registry: Arc::clone(&self.0.retained_registry),
            temporary_values: Arc::clone(&self.0.temporary_values),
            retained_diagnostics: Arc::clone(&self.0.retained_diagnostics),
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
            retained_diagnostics: Arc::new(retained_diagnostics::RetainedDiagnostics::new(
                limits.retained_diagnostics.clone(),
            )),
            temporary_values: Arc::new(temporary_values::TemporaryValues::new(
                limits.temporaries.clone(),
            )),
            retained_registry: Arc::new(retained_registry::RetainedRegistry::new(
                limits.retained_registry.clone(),
            )),
            retained_names: Arc::new(retained_names::RetainedNames::new(
                limits.retained_names.clone(),
            )),
            retained_definitions: Arc::new(retained_definitions::RetainedDefinitions::new(
                limits.retained_definitions.clone(),
            )),
            retained_values: Arc::new(retained_values::RetainedValues::new(
                limits.retained_values.clone(),
            )),
            limits,
            control,
            used: Arc::new(AtomicU64::new(0)),
            active: AtomicUsize::new(0),
            output: Arc::new(AtomicUsize::new(0)),
            imports: Arc::new(Mutex::new([0; 5])),
            snapshots: Arc::new(Mutex::new([0; 2])),
            cleanup_steps: Arc::new(AtomicU64::new(0)),
            cleaning: false,
            stopped: Mutex::new(None),
        }))
    }
    pub(crate) fn shared(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
    pub(crate) fn limits(&self) -> &RunLimits {
        &self.0.limits
    }
    pub(crate) fn control(&self) -> &OperationControl {
        &self.0.control
    }

    pub(crate) fn used(&self) -> u64 {
        self.0
            .cleanup_steps
            .load(Ordering::Relaxed)
            .saturating_add(if self.0.cleaning {
                0
            } else {
                self.0.used.load(Ordering::Relaxed)
            })
    }
    pub(crate) fn stop(&self, error: BWErr) -> Diagnostic {
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
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < self.0.limits.steps).then(|| used + 1)
            })
            .map_err(|_| self.limit("evaluation steps", self.0.limits.steps))?;
        if self.0.cleaning {
            let _ = self
                .0
                .cleanup_steps
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                    Some(used.saturating_add(1))
                });
        }
        Ok(())
    }

    pub(crate) fn enter(&self) -> DiagnosticResult<EvaluationGuard> {
        self.checkpoint()?;
        self.0
            .active
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                (active < self.0.limits.evaluation_depth).then(|| active + 1)
            })
            .map_err(|_| self.limit("evaluation depth", self.0.limits.evaluation_depth as u64))?;
        let guard = EvaluationGuard(self.shared());
        // Depth alone cannot bound the stack: frame sizes vary by target and
        // profile, and hosts choose their threads' stacks.
        if stack::short_of(stack::LEVEL) {
            return Err(self.limit(stack::RESOURCE, stack::LEVEL as u64));
        }
        Ok(guard)
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
