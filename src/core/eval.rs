use pest::iterators::Pair;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::Arc,
};

mod blocking;
mod builtins;
mod cleanup;
mod compiled;
pub use compiled::{CompiledModules, CompiledStatistics, DEFAULT_COMPILED_SOURCE_BYTES};
mod fixtures;
pub use fixtures::{evaluate_suite_fixture_async, FixtureInputs, FixtureResult};
mod diagnostics;
pub(crate) mod execution;
mod filesystem;
mod imports;
mod output;
mod polling;
mod results;
mod snapshots;
mod temporaries;
pub(crate) use imports::{playwright, webdriver};
use imports::{LoadedModule, ModuleCache};
use temporaries::TemporaryArguments;

use super::{
    ast::{
        self, AccessSegment, AssignmentValue, BinaryOp, Block, Call, Definition, ElseBranch, Expr,
        ExprKind, Name, Node, Program, Span, Statement, StatementKind, UnaryOp,
    },
    diagnostic::{CallFrame, Diagnostic, DiagnosticResult},
    grammar::{finite_float, validate_numeric_values, BWErr, Literal, LiteralResult, Rule},
    operation::{NativeOperation, OperationControl},
    run::{
        DefinitionReservation, EvaluationGuard, RegistryPlan, RegistryReservation, RetainedName,
        RunBudget, RunEnvironment, RunLimits, RuntimeDiagnostic, SourceFailure, StoredCallFrame,
        StoredDiagnostic, StoredValue, TemporaryValue, ValueReservation,
    },
    signature::{StatementOrigin, StatementSignature},
    value_limits::Owned,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod execution_contract;

#[cfg(test)]
mod native_contract;

#[derive(Debug)]
enum Completion {
    Normal(TemporaryValue),
    Return(TemporaryValue),
    Break,
    Continue,
}

pub(crate) type EvaluationResult<T> = Result<T, RuntimeDiagnostic>;
type CompletionResult = EvaluationResult<Completion>;
type RuntimeResult = DiagnosticResult<Literal>;
type TemporaryResult = EvaluationResult<TemporaryValue>;

#[derive(Clone)]
enum StmtType {
    Operation {
        operation: NativeOperation,
        builtin: bool,
        _registry: Option<Arc<RegistryReservation>>,
    },
    Native {
        body: NativeBody,
        metadata: Arc<StatementSignature>,
        _registry: Option<Arc<RegistryReservation>>,
    },
    UserDefined {
        definition: Arc<Definition>,
        metadata: Arc<StatementSignature>,
        _reservation: Option<Arc<DefinitionReservation>>,
        _registry: Option<Arc<RegistryReservation>>,
    },
    Imported {
        module: Arc<LoadedModule>,
        exported: Arc<str>,
        metadata: Arc<StatementSignature>,
        import_site: Span,
        _registry: Option<Arc<RegistryReservation>>,
    },
}

impl StmtType {
    fn registry(&self) -> Option<&Arc<RegistryReservation>> {
        match self {
            Self::Native { _registry, .. }
            | Self::Operation { _registry, .. }
            | Self::UserDefined { _registry, .. }
            | Self::Imported { _registry, .. } => _registry.as_ref(),
        }
    }
    fn metadata(&self) -> &StatementSignature {
        match self {
            Self::Operation { operation, .. } => operation.signature(),
            Self::Native { metadata, .. }
            | Self::UserDefined { metadata, .. }
            | Self::Imported { metadata, .. } => metadata,
        }
    }
}

#[derive(Clone)]
struct StoredNamespace {
    span: Span,
    _registry: Option<Arc<RegistryReservation>>,
}

#[derive(Default)]
struct Frame {
    variables: HashMap<RetainedName, Arc<StoredValue>>,
    statements: HashMap<Arc<str>, StmtType>,
    namespaces: HashMap<Arc<str>, StoredNamespace>,
    dependency_depth: usize,
    // Definitions are not first-class values, so lexical owners remain on the stack.
    parent: Option<usize>,
}

#[derive(Clone)]
struct HandledError {
    invocation: usize,
    diagnostic: Arc<StoredDiagnostic>,
}

/// Execution state. Infallible Clone is a host-owned copy outside snapshot work
/// admission; use try_clone for checked table/path copying. Stored payloads and
/// live retention trackers are shared; work counters and stop latches are copied.
#[derive(Clone)]
pub struct Context {
    frames: Vec<Frame>,
    current: usize,
    calls: Vec<Arc<StoredCallFrame>>,
    handlers: Vec<HandledError>,
    modules: ModuleCache,
    loading: Vec<PathBuf>,
    pub(crate) working_directory: Result<PathBuf, Arc<io::Error>>,
    pub(crate) environment: Option<Arc<RunEnvironment>>,
    pub(crate) budget: Option<RunBudget>,
    pub(crate) asynchronous: bool,
    worker_control: Option<OperationControl>,
    trace_statements: bool,
    #[cfg(test)]
    expression_visits: std::cell::RefCell<Vec<String>>,
}

impl Default for Context {
    fn default() -> Self {
        Self::with_directory_snapshot(std::env::current_dir().map_err(Arc::new))
    }
}

impl Context {
    pub(crate) fn with_directory_snapshot(
        working_directory: Result<PathBuf, Arc<io::Error>>,
    ) -> Self {
        Self {
            frames: vec![Frame::default()],
            current: 0,
            calls: vec![],
            handlers: vec![],
            modules: ModuleCache::default(),
            loading: vec![],
            working_directory,
            environment: None,
            budget: Some(RunBudget::new(
                RunLimits::default(),
                OperationControl::default(),
            )),
            asynchronous: false,
            worker_control: None,
            trace_statements: false,
            #[cfg(test)]
            expression_visits: Default::default(),
        }
    }
}

impl Context {
    /// Trace entry-script statements through the same bounded stderr output path as CLI --debug.
    pub fn set_statement_tracing(&mut self, enabled: bool) {
        self.trace_statements = enabled;
    }

    fn check_execution_mode(&self) -> EvaluationResult<()> {
        self.checkpoint()?;
        let reason = if self.asynchronous {
            tokio::runtime::Handle::try_current()
                .err()
                .map(|_| "Evaluate asynchronous programs inside a Tokio runtime")
        } else if self.frames.iter().any(|frame| {
            frame
                .statements
                .values()
                .any(|statement| matches!(statement, StmtType::Operation { builtin: false, .. }))
        }) {
            Some("This context contains NativeOperation registrations; use asynchronous program execution")
        } else {
            None
        };
        match reason {
            Some(reason) => Err(self.detail_error(BWErr::AsyncRuntime, reason, None, false)),
            None => Ok(()),
        }
    }

    /// Install an async, cooperative blocking, or isolated native operation.
    /// Synchronous program entry points reject contexts containing these registrations before effects.
    pub fn register_operation(&mut self, operation: NativeOperation) -> DiagnosticResult<()> {
        let metadata = operation.signature();
        self.check_statement_collision(metadata.normalized(), metadata.header())
            .map_err(RuntimeDiagnostic::into_diagnostic)?;
        let registry = self
            .reserve_registry(RegistryPlan::signature(metadata))
            .map_err(|error| {
                self.runtime_diagnostic(error.into(), Some(metadata.header()), false)
                    .into_diagnostic()
            })?;
        let key = registry.as_ref().map_or_else(
            || Arc::from(metadata.normalized()),
            |reservation| Arc::clone(&reservation.key),
        );
        let header = metadata.header().clone();
        self.insert_statement(
            &key,
            &header,
            StmtType::Operation {
                operation,
                builtin: false,
                _registry: registry,
            },
        )
        .map_err(RuntimeDiagnostic::into_diagnostic)
    }
    /// A fresh context with local runtime budgets and an externally cancellable control.
    /// Counters persist across evaluations; use a fresh context after budget exhaustion.
    pub fn with_control(limits: RunLimits, control: OperationControl) -> DiagnosticResult<Self> {
        control.checkpoint()?;
        limits.validate()?;
        Ok(Self {
            budget: Some(RunBudget::new(limits, control)),
            ..Self::default()
        })
    }

    pub fn with_limits(limits: RunLimits) -> DiagnosticResult<Self> {
        Self::with_control(limits, OperationControl::default())
    }

    fn enter_evaluation(&self) -> DiagnosticResult<Option<EvaluationGuard>> {
        self.budget.as_ref().map(RunBudget::enter).transpose()
    }

    /// Observe cancellation/deadline/latched limit failures without consuming a step.
    pub fn checkpoint(&self) -> DiagnosticResult<()> {
        self.budget.as_ref().map_or(Ok(()), RunBudget::checkpoint)?;
        self.worker_control
            .as_ref()
            .map_or(Ok(()), OperationControl::checkpoint)
    }

    fn tick(&self) -> DiagnosticResult<()> {
        self.budget.as_ref().map_or(Ok(()), RunBudget::tick)
    }

    fn check_call_depth(&self) -> DiagnosticResult<()> {
        self.checkpoint()?;
        if let Some(budget) = &self.budget {
            if self.calls.len() >= budget.limits().call_depth {
                return Err(budget.limit("call depth", budget.limits().call_depth as u64));
            }
        }
        Ok(())
    }

    fn check_import_depth(&self) -> DiagnosticResult<()> {
        self.checkpoint()?;
        if let Some(budget) = &self.budget {
            if self.loading.len() >= budget.limits().import_depth {
                return Err(budget.limit(
                    "import initialization depth",
                    budget.limits().import_depth as u64,
                ));
            }
            budget.check_parser_entry()?;
        }
        Ok(())
    }

    pub(crate) fn check_source_size(&self, bytes: usize) -> DiagnosticResult<()> {
        self.checkpoint()?;
        if let Some(budget) = &self.budget {
            if bytes > budget.limits().source_bytes {
                return Err(budget.limit("source bytes", budget.limits().source_bytes as u64));
            }
        } else if bytes > super::syntax_limits::DEFAULT_SOURCE_BYTES {
            return Err(BWErr::ResourceLimit {
                resource: "source bytes",
                limit: super::syntax_limits::DEFAULT_SOURCE_BYTES as u64,
            }
            .into());
        }
        Ok(())
    }

    fn retain_limit(&self, error: Diagnostic) -> Diagnostic {
        self.latch_limit(&error);
        error
    }

    fn latch_limit(&self, error: &Diagnostic) {
        if let (Some(budget), BWErr::ResourceLimit { resource, limit }) =
            (&self.budget, error.error.as_ref())
        {
            budget.limit(resource, *limit);
        }
    }

    fn limits(&self) -> RunLimits {
        self.budget
            .as_ref()
            .map(|budget| budget.limits().clone())
            .unwrap_or_default()
    }

    fn check_value(&self, value: &Literal) -> DiagnosticResult<()> {
        self.checkpoint()?;
        self.limits()
            .values
            .check(value)
            .map(|_| ())
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))
    }

    fn reserve_value(&self, value: &Literal) -> DiagnosticResult<Option<ValueReservation>> {
        self.checkpoint()?;
        let size = self
            .limits()
            .values
            .check(value)
            .map_err(|error| self.retain_limit(Diagnostic::new(error)))?;
        self.budget
            .as_ref()
            .map(|budget| budget.reserve_value(size))
            .transpose()
    }

    fn store_value(&self, value: Literal) -> DiagnosticResult<Arc<StoredValue>> {
        let value = Owned::new(value);
        let reservation = self.reserve_value(&value)?;
        Ok(Arc::new(StoredValue::new(value.into_inner(), reservation)))
    }

    fn retain_name(&self, name: &str) -> DiagnosticResult<RetainedName> {
        self.budget.as_ref().map_or_else(
            || Ok(RetainedName::untracked(name)),
            |budget| budget.retain_name(name),
        )
    }

    fn variable_key(&self, name: &str, frame: usize) -> DiagnosticResult<RetainedName> {
        self.checkpoint()?;
        match self.frames[frame].variables.get_key_value(name) {
            Some((key, _)) => Ok(key.clone()),
            None => self.retain_name(name),
        }
    }

    fn reserve_registry(
        &self,
        plan: RegistryPlan<'_>,
    ) -> DiagnosticResult<Option<Arc<RegistryReservation>>> {
        self.budget
            .as_ref()
            .map(|budget| budget.reserve_registry(plan))
            .transpose()
    }

    /// Native templates are host-owned; each Engine run admits them locally.
    pub(crate) fn admit_native_registry(&mut self) -> EvaluationResult<()> {
        let mut admitted = Vec::new();
        let mut entries: Vec<_> = self.frames[0].statements.iter().collect();
        entries.sort_unstable_by_key(|(left, _)| *left);
        for (name, statement) in entries {
            if statement.registry().is_some()
                && matches!(
                    statement,
                    StmtType::Native { .. } | StmtType::Operation { .. }
                )
            {
                let metadata = statement.metadata();
                let reservation = self
                    .reserve_registry(RegistryPlan::signature(metadata).with_key(Arc::clone(name)))
                    .map_err(|error| {
                        self.runtime_diagnostic(error.into(), Some(metadata.header()), false)
                    })?;
                admitted.push((Arc::clone(name), reservation));
            }
        }
        self.checkpoint()?;
        for (name, reservation) in admitted {
            if let StmtType::Native { _registry, .. } | StmtType::Operation { _registry, .. } = self
                .frames[0]
                .statements
                .get_mut(name.as_ref())
                .expect("native template")
            {
                *_registry = reservation;
            }
        }
        Ok(())
    }

    pub(crate) fn check_syntax(&self, name: &str, source: &str) -> EvaluationResult<()> {
        let limits = self
            .budget
            .as_ref()
            .map(|budget| budget.limits().clone())
            .unwrap_or_default();
        ast::check_source_with_reporter(
            name,
            source,
            limits.source_bytes,
            &limits.syntax,
            |failure| self.ast_error(failure),
        )
        .inspect_err(|error| self.latch_limit(error))
    }

    /// Parse an imported module, sharing the tree through the run's compiled
    /// module cache when it has one. Limits are checked exactly as a fresh parse.
    pub(crate) fn parse_module(&self, name: &str, source: &str) -> EvaluationResult<Arc<Program>> {
        let Some(modules) = self
            .environment
            .as_ref()
            .and_then(|environment| environment.compiled.as_ref())
        else {
            return self.parse_source(name, source).map(Arc::new);
        };
        if let Some(budget) = &self.budget {
            budget.check_parser_entry()?;
        }
        let limits = self
            .budget
            .as_ref()
            .map(|budget| budget.limits().clone())
            .unwrap_or_default();
        modules.get_or_parse(
            name,
            source,
            compiled::ParseLimits {
                source_bytes: limits.source_bytes,
                syntax: limits.syntax,
                ast: limits.ast,
            },
            || self.parse_source(name, source),
        )
    }

    pub(crate) fn parse_source(&self, name: &str, source: &str) -> EvaluationResult<Program> {
        if let Some(budget) = &self.budget {
            budget.check_parser_entry()?;
        }
        let limits = self
            .budget
            .as_ref()
            .map(|budget| budget.limits().clone())
            .unwrap_or_default();
        Program::parse_with_reporter(
            name,
            source,
            limits.source_bytes,
            &limits.syntax,
            &limits.ast,
            |failure| self.ast_error(failure),
        )
        .inspect_err(|error| self.latch_limit(error))
    }

    pub(crate) fn read_source(&self, path: &std::path::Path) -> Result<String, SourceFailure> {
        self.checkpoint().map_err(SourceFailure::Diagnostic)?;
        let bytes = super::run::read_source(
            path,
            Some(
                self.budget
                    .as_ref()
                    .map_or(super::syntax_limits::DEFAULT_SOURCE_BYTES, |budget| {
                        budget.limits().source_bytes
                    }),
            ),
        )?;
        self.check_source_size(bytes.len())
            .map_err(SourceFailure::Diagnostic)?;
        String::from_utf8(bytes).map_err(|error| {
            SourceFailure::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        })
    }

    pub(crate) fn register_run_native(
        &mut self,
        signature: StatementSignature,
        callback: impl Fn(&[Literal], &RunEnvironment) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        if signature.origin() != StatementOrigin::Native {
            return Err(self
                .detail_error(
                    BWErr::SignatureError,
                    "Native registration requires a native signature",
                    Some(signature.header()),
                    false,
                )
                .into_diagnostic());
        }
        self.insert_native(
            signature,
            Arc::new(move |values, context| {
                let environment = context.environment.as_ref().ok_or_else(|| {
                    context.detail_error(
                        BWErr::RunConfiguration,
                        "Native operation needs a configured run",
                        context.calls.last().map(|record| &record.frame.call_site),
                        false,
                    )
                })?;
                callback(values, environment).map_err(RuntimeDiagnostic::from)
            }),
        )
    }

    /// Validate and install owned values in the entry script's root scope.
    /// Replaces matching root bindings only after every input is valid.
    /// Imported modules keep independent globals; local bindings can shadow inputs.
    pub fn set_input_variables(
        &mut self,
        variables: BTreeMap<String, Literal>,
    ) -> DiagnosticResult<()> {
        self.set_input_variables_runtime(variables)
            .map_err(RuntimeDiagnostic::into_diagnostic)
    }

    pub(crate) fn set_input_variables_runtime(
        &mut self,
        variables: BTreeMap<String, Literal>,
    ) -> EvaluationResult<()> {
        let variables = Owned::new(variables);
        self.checkpoint()?;
        for (name, value) in variables.iter() {
            if let Some(budget) = &self.budget {
                budget.check_name_length(name.len())?;
            }
            super::input::validate_name_with("host variables", name, |message| {
                self.formatted_error(BWErr::InputError, message, None, false)
            })?;
            self.check_value(value)?;
            validate_numeric_values(value).map_err(|_| {
                self.formatted_error(
                    BWErr::InputError,
                    format_args!(
                        "host variables: {name:?}: values must contain only finite floats"
                    ),
                    None,
                    false,
                )
            })?;
        }
        self.checkpoint()?;
        // RetainedName hashes only immutable text; its counters are lifetime metadata.
        #[allow(clippy::mutable_key_type)]
        let bindings = variables
            .into_inner()
            .into_iter()
            .map(|(name, value)| {
                let value = self.store_value(value)?;
                self.variable_key(&name, 0).map(|name| (name, value))
            })
            .collect::<DiagnosticResult<HashMap<_, _>>>()?;
        self.checkpoint()?;
        self.frames[0].variables.extend(bindings);
        Ok(())
    }

    #[cfg(test)]
    fn get_variable(&self, name: &str) -> LiteralResult {
        let value = self
            .get_variable_ref(name)
            .map_err(Diagnostic::into_error)?;
        self.check_value(value).map_err(Diagnostic::into_error)?;
        Ok(value.clone())
    }

    #[cfg(test)]
    fn get_variable_ref(&self, name: &str) -> DiagnosticResult<&Literal> {
        self.get_variable_binding(name, None)
            .map(|binding| &binding.value)
            .map_err(RuntimeDiagnostic::into_diagnostic)
    }

    fn get_variable_binding(
        &self,
        name: &str,
        span: Option<&Span>,
    ) -> EvaluationResult<&Arc<StoredValue>> {
        self.find_variable_binding(name)
            .ok_or_else(|| self.undefined_variable(name, span))
    }

    fn find_variable_binding(&self, name: &str) -> Option<&Arc<StoredValue>> {
        let mut index = Some(self.current);
        while let Some(frame_index) = index {
            let frame = &self.frames[frame_index];
            if let Some(value) = frame.variables.get(name) {
                return Some(value);
            }
            index = frame.parent;
        }
        None
    }

    fn set_variable(
        &mut self,
        name: &str,
        literal: Literal,
    ) -> DiagnosticResult<Option<Arc<StoredValue>>> {
        let value = self.store_value(literal)?;
        let name = self.variable_key(name, self.current)?;
        Ok(self.frames[self.current].variables.insert(name, value))
    }

    fn get_statement(&self, signature: &str) -> Option<(StmtType, usize)> {
        self.get_statement_ref(signature)
            .map(|(statement, owner)| (statement.clone(), owner))
    }

    fn get_statement_ref(&self, signature: &str) -> Option<(&StmtType, usize)> {
        let mut index = Some(self.current);
        while let Some(frame_index) = index {
            let frame = &self.frames[frame_index];
            if let Some(statement) = frame.statements.get(signature) {
                return Some((statement, frame_index));
            }
            if signature
                .split_once("::")
                .is_some_and(|(namespace, _)| frame.namespaces.contains_key(namespace))
            {
                return None;
            }
            index = frame.parent;
        }
        None
    }

    #[cfg(test)]
    fn with_invocation(
        &mut self,
        frame: Frame,
        body: impl FnOnce(&mut Self) -> CompletionResult,
    ) -> CompletionResult {
        let caller = self.current;
        self.current = self.frames.len();
        self.frames.push(frame);
        // All language outcomes, including errors, restore the dynamic caller.
        let result = body(self);
        self.frames.pop();
        self.current = caller;
        result
    }

    /// Register a native sentence header such as `Read |path|` in this scope.
    /// Arguments are evaluated once in caller order before the callback runs.
    /// Async runs use workers; callbacks return owned finite values or typed errors.
    /// Invalid headers and collisions leave the existing registry unchanged.
    /// Cloning a context shares callback captures but copies DSL bindings.
    pub fn register_native(
        &mut self,
        header: &str,
        callback: impl Fn(&[Literal]) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        self.register_native_with_signature(StatementSignature::native(header)?, callback)
    }

    /// Register a validated signature whose kinds are enforced before callback entry
    /// and before publishing the return value. Error documentation is advisory.
    pub fn register_native_with_signature(
        &mut self,
        signature: StatementSignature,
        callback: impl Fn(&[Literal]) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        if signature.origin() != StatementOrigin::Native {
            return Err(self
                .detail_error(
                    BWErr::SignatureError,
                    "Native registration requires a native signature",
                    Some(signature.header()),
                    false,
                )
                .into_diagnostic());
        }
        self.insert_native(
            signature,
            Arc::new(move |values, _| callback(values).map_err(RuntimeDiagnostic::from)),
        )
    }

    /// Visible metadata in normalized order, with lexical shadowing resolved.
    pub fn statement_signatures(&self) -> Vec<&StatementSignature> {
        let mut visible = BTreeMap::new();
        let mut hidden = HashSet::new();
        let mut index = Some(self.current);
        while let Some(owner) = index {
            let frame = &self.frames[owner];
            for (key, statement) in &frame.statements {
                if key
                    .split_once("::")
                    .is_some_and(|(namespace, _)| hidden.contains(namespace))
                {
                    continue;
                }
                visible.entry(key).or_insert_with(|| statement.metadata());
            }
            hidden.extend(frame.namespaces.keys().map(AsRef::as_ref));
            index = frame.parent;
        }
        visible.into_values().collect()
    }

    /// Resolve metadata for a complete header, using normal sentence matching.
    pub fn statement_signature(
        &self,
        header: &str,
    ) -> DiagnosticResult<Option<&StatementSignature>> {
        let query = ast::native_signature("<signature query>", header)?;
        Ok(self
            .get_statement_ref(&query.signature)
            .map(|(statement, _)| statement.metadata()))
    }

    /// Metadata for hover at a parsed call in the current lexical environment.
    /// It takes a syntax-tree node, which is not part of the embedding API.
    #[doc(hidden)]
    pub fn signature_for_call(&self, call: &Call) -> Option<&StatementSignature> {
        self.get_statement_ref(&call.signature)
            .map(|(statement, _)| statement.metadata())
    }

    /// Complete an initial sentence prefix before its first parameter.
    /// Results use the same normalized ordering and shadowing as registry lookup.
    pub fn complete_statements(&self, prefix: &str) -> Vec<&StatementSignature> {
        // Normalize without copying an unbounded host prefix. A normalized
        // prefix longer than every registered key cannot match any statement.
        let maximum = self
            .frames
            .iter()
            .flat_map(|frame| frame.statements.keys())
            .map(|key| key.len())
            .max()
            .unwrap_or(0);
        let mut normalized = String::new();
        for character in prefix
            .split('|')
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|character| !matches!(character, ' ' | '\t'))
            .flat_map(char::to_lowercase)
        {
            if normalized
                .len()
                .checked_add(character.len_utf8())
                .is_none_or(|length| length > maximum)
            {
                return vec![];
            }
            normalized.push(character);
        }
        self.statement_signatures()
            .into_iter()
            .filter(|signature| signature.normalized().starts_with(&normalized))
            .collect()
    }

    #[cfg(test)]
    fn register_callback(
        &mut self,
        name: &str,
        header: &str,
        callback: Callback,
    ) -> DiagnosticResult<()> {
        self.insert_native(StatementSignature::native_at(name, header)?, callback)
    }

    fn insert_native(
        &mut self,
        signature: StatementSignature,
        callback: Callback,
    ) -> DiagnosticResult<()> {
        self.check_statement_collision(signature.normalized(), signature.header())
            .map_err(RuntimeDiagnostic::into_diagnostic)?;
        let registry = self
            .reserve_registry(RegistryPlan::signature(&signature))
            .map_err(|error| {
                self.runtime_diagnostic(error.into(), Some(signature.header()), false)
                    .into_diagnostic()
            })?;
        let metadata = Arc::new(signature);
        self.insert_statement(
            metadata.normalized(),
            metadata.header(),
            StmtType::Native {
                body: NativeBody::Callback(callback),
                metadata: Arc::clone(&metadata),
                _registry: registry,
            },
        )
        .map_err(RuntimeDiagnostic::into_diagnostic)
    }

    fn insert_statement(
        &mut self,
        signature: &str,
        span: &Span,
        statement: StmtType,
    ) -> EvaluationResult<()> {
        self.check_statement_collision(signature, span)?;
        let key = statement.registry().map_or_else(
            || Arc::from(signature),
            |reservation| Arc::clone(&reservation.key),
        );
        self.frames[self.current].statements.insert(key, statement);
        Ok(())
    }

    fn check_statement_collision(&self, signature: &str, span: &Span) -> EvaluationResult<()> {
        if let Some((namespace, _)) = signature.split_once("::") {
            if let Some(original) = self.frames[self.current].namespaces.get(namespace) {
                return Err(imports::namespace_collision(
                    self,
                    namespace,
                    &original.span,
                    span,
                ));
            }
        }
        let statements = &self.frames[self.current].statements;
        if let Some(original) = statements.get(signature) {
            let origin = match original {
                StmtType::Native { metadata, .. } => (metadata.header(), true),
                StmtType::Operation { operation, .. } => (operation.signature().header(), true),
                StmtType::UserDefined { definition, .. } => (&definition.span, false),
                StmtType::Imported { metadata, .. } => (metadata.header(), false),
            };
            return Err(self.duplicate_error(
                |[signature, original, duplicate]| BWErr::DuplicateStatement {
                    signature,
                    original,
                    duplicate,
                },
                signature,
                origin,
                span,
                "first definition",
            ));
        }
        Ok(())
    }

    /// Fill vacant built-in slots, preserving existing registrations.
    pub fn init_statements(&mut self) {
        builtins::initialize(self);
    }

    fn with_call(
        &mut self,
        signature: &str,
        statement: Option<&Span>,
        call_site: &Span,
        definition_site: Option<&Span>,
        body: impl FnOnce(&mut Self) -> TemporaryResult,
    ) -> TemporaryResult {
        self.check_call_depth()
            .map_err(|error| self.runtime_diagnostic(error.into(), Some(call_site), false))?;
        let frame = self.retain_call(signature, statement, call_site, definition_site)?;
        self.calls.push(frame);
        let result = body(self).map_err(|error| self.runtime_diagnostic(error, None, false));
        self.calls.pop();
        result
    }
}

type Callback = Arc<dyn Fn(&[Literal], &mut Context) -> EvaluationResult<Literal> + Send + Sync>;

#[derive(Clone)]
enum NativeBody {
    Callback(Callback),
    Builtin(builtins::Builtin),
}

fn write_log(value: &Literal, output: &mut impl Write, context: &Context) -> EvaluationResult<()> {
    context.write_value_record(value, output, true).map(|_| ())
}

// Synchronous entry points poll the same evaluator once. No runtime or blocking
// executor is required; async-only dispatch is rejected before any effects.
fn sync_result<T>(
    future: impl std::future::Future<Output = EvaluationResult<T>>,
) -> EvaluationResult<T> {
    let mut future = std::pin::pin!(future);
    let mut task = std::task::Context::from_waker(std::task::Waker::noop());
    match std::future::Future::poll(future.as_mut(), &mut task) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => Err(BWErr::AsyncRuntime(
            "Use the asynchronous execution API for this operation".into(),
        )
        .into()),
    }
}

#[cfg(test)]
fn invoke(call: &Call, context: &mut Context) -> TemporaryResult {
    sync_result(execution::invoke(call, context))
}
fn evaluate_expression(expression: &Expr, context: &mut Context) -> TemporaryResult {
    sync_result(execution::evaluate_expression(expression, context))
}
fn evaluate_block(block: &Block, context: &mut Context) -> CompletionResult {
    sync_result(execution::evaluate_block(block, context))
}
#[cfg(test)]
fn evaluate_statement(statement: &Statement, context: &mut Context) -> CompletionResult {
    sync_result(execution::evaluate_statement(statement, context))
}

// Retain runtime boundary guards even though public entry points validate placement.
fn finish_script(completion: Completion, context: &Context, span: &Span) -> TemporaryResult {
    let reason = match completion {
        Completion::Normal(value) => return Ok(value),
        Completion::Return(_) => "Return requires a custom-statement body",
        Completion::Break => "Break requires an enclosing loop in the same invocation",
        Completion::Continue => "Continue requires an enclosing loop in the same invocation",
    };
    Err(context.detail_error(BWErr::ControlFlowError, reason, Some(span), false))
}

#[doc(hidden)]
/// Evaluate an already parsed, owned statement at script level in this context.
///
/// Definitions retain their syntax tree and source spans after the program is dropped.
/// The entire statement is validated before execution; custom calls consume their returns.
pub fn execute_statement(statement: &Statement, context: &mut Context) -> LiteralResult {
    execute_statement_detailed(statement, context).map_err(Diagnostic::into_error)
}

#[doc(hidden)]
/// Execute one script-level statement with source locations and entered-call frames.
pub fn execute_statement_detailed(statement: &Statement, context: &mut Context) -> RuntimeResult {
    execute_statement_runtime(statement, context).map_err(RuntimeDiagnostic::into_diagnostic)
}

fn execute_statement_runtime(
    statement: &Statement,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    sync_result(execution::execute_statement_runtime(statement, context))
}

/// Validate the complete program, then execute without parsing or rebuilding it.
pub fn evaluate_program(program: &Program, context: &mut Context) -> LiteralResult {
    evaluate_program_detailed(program, context).map_err(Diagnostic::into_error)
}

/// Validate and execute a program while preserving structured diagnostic causes.
pub fn evaluate_program_detailed(program: &Program, context: &mut Context) -> RuntimeResult {
    evaluate_program_runtime(program, context).map_err(RuntimeDiagnostic::into_diagnostic)
}

/// Execute using an owned context so dropping the future drops all suspended DSL state.
/// Ordinary native callbacks and output run on bounded workers. Blocking work must
/// cooperate with cancellation; NativeOperation additionally supports explicit adapters.
pub fn evaluate_program_async(
    program: &Program,
    mut context: Context,
) -> impl std::future::Future<Output = RuntimeResult> + '_ {
    context.asynchronous = true;
    async move {
        let result = execution::evaluate_program_runtime(program, &mut context).await;
        context.after_evaluation(result).map_err(|error| {
            context
                .runtime_diagnostic(error, None, false)
                .into_diagnostic()
        })
    }
}

pub(crate) fn evaluate_program_runtime(
    program: &Program,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    sync_result(execution::evaluate_program_runtime(program, context))
}

#[doc(hidden)]
/// Compatibility entry point for callers that already hold a Pest pair.
///
/// This lowers the pair once. Prefer `Program::parse` and `evaluate_program` to
/// share one owned source allocation across an entire script.
pub fn botwork(pair: Pair<Rule>, context: &mut Context) -> LiteralResult {
    botwork_detailed(pair, context).map_err(Diagnostic::into_error)
}

#[doc(hidden)]
/// Parser-pair compatibility with detailed execution errors.
pub fn botwork_detailed(pair: Pair<Rule>, context: &mut Context) -> RuntimeResult {
    let result = (|| {
        context.check_execution_mode()?;
        let node = ast::from_pair_with_reporter(pair, |failure| context.ast_error(failure))?;
        let limits = context.limits();
        super::ast_limits::check_node(&node, &limits.ast, limits.source_bytes)
            .map_err(|failure| context.ast_error(failure))?;
        match node {
            Node::Statement(statement) => execute_statement_runtime(&statement, context),
            Node::Expression(expression) => {
                evaluate_expression(&expression, context).map(TemporaryValue::into_inner)
            }
            Node::Block(block) => {
                ast::validate_control_script(&block.statements)
                    .map_err(|failure| context.ast_error(failure))?;
                finish_script(evaluate_block(&block, context)?, context, &block.span)
                    .map(TemporaryValue::into_inner)
            }
            Node::None => context
                .temporary(Literal::None)
                .map(TemporaryValue::into_inner),
        }
    })();
    result.map_err(|error| {
        context
            .runtime_diagnostic(error, None, false)
            .into_diagnostic()
    })
}
