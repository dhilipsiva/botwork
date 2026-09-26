use pest::iterators::Pair;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    sync::Arc,
};

mod diagnostics;
mod imports;
mod results;
mod snapshots;
mod temporaries;
use imports::{LoadedModule, ModuleCache};
use temporaries::TemporaryArguments;

use super::{
    ast::{
        self, AccessSegment, AssignmentValue, BinaryOp, Block, Call, Definition, ElseBranch, Expr,
        ExprKind, Name, Node, Program, Span, Statement, StatementKind, UnaryOp,
    },
    diagnostic::{CallFrame, Diagnostic, DiagnosticResult},
    grammar::{finite_float, validate_value, BWErr, Literal, LiteralResult, Rule},
    operation::OperationControl,
    run::{
        DefinitionReservation, EvaluationGuard, RegistryPlan, RegistryReservation, RetainedName,
        RunBudget, RunEnvironment, RunLimits, SourceFailure, StoredCallFrame, StoredDiagnostic,
        StoredValue, TemporaryValue, ValueReservation,
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

type CompletionResult = DiagnosticResult<Completion>;
type RuntimeResult = DiagnosticResult<Literal>;
type TemporaryResult = DiagnosticResult<TemporaryValue>;

#[derive(Clone)]
enum StmtType {
    Native {
        callback: Callback,
        builtin_log: bool,
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
            | Self::UserDefined { _registry, .. }
            | Self::Imported { _registry, .. } => _registry.as_ref(),
        }
    }
    fn metadata(&self) -> &StatementSignature {
        match self {
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
    pub(crate) working_directory: Result<PathBuf, String>,
    pub(crate) environment: Option<Arc<RunEnvironment>>,
    pub(crate) budget: Option<RunBudget>,
    #[cfg(test)]
    expression_visits: std::cell::RefCell<Vec<String>>,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            frames: vec![Frame::default()],
            current: 0,
            calls: vec![],
            handlers: vec![],
            modules: ModuleCache::default(),
            loading: vec![],
            working_directory: std::env::current_dir().map_err(|error| error.to_string()),
            environment: None,
            budget: Some(RunBudget::new(
                RunLimits::default(),
                OperationControl::default(),
            )),
            #[cfg(test)]
            expression_visits: Default::default(),
        }
    }
}

impl Context {
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
        self.budget.as_ref().map_or(Ok(()), RunBudget::checkpoint)
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
        if let (Some(budget), BWErr::ResourceLimit { resource, limit }) =
            (&self.budget, error.error.as_ref())
        {
            budget.limit(resource, *limit);
        }
        error
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
    pub(crate) fn admit_native_registry(&mut self) -> DiagnosticResult<()> {
        let mut admitted = Vec::new();
        let mut entries: Vec<_> = self.frames[0].statements.iter().collect();
        entries.sort_unstable_by_key(|(left, _)| *left);
        for (name, statement) in entries {
            if let StmtType::Native {
                metadata,
                _registry: Some(_),
                ..
            } = statement
            {
                let reservation = self
                    .reserve_registry(RegistryPlan::signature(metadata).with_key(Arc::clone(name)))
                    .map_err(|error| error.at(metadata.header()))?;
                admitted.push((Arc::clone(name), reservation));
            }
        }
        self.checkpoint()?;
        for (name, reservation) in admitted {
            if let StmtType::Native { _registry, .. } = self.frames[0]
                .statements
                .get_mut(name.as_ref())
                .expect("native template")
            {
                *_registry = reservation;
            }
        }
        Ok(())
    }

    pub(crate) fn check_syntax(&self, name: &str, source: &str) -> DiagnosticResult<()> {
        let limits = self
            .budget
            .as_ref()
            .map(|budget| budget.limits().clone())
            .unwrap_or_default();
        ast::check_source(name, source, limits.source_bytes, &limits.syntax)
            .map_err(|error| self.retain_limit(error))
    }

    pub(crate) fn parse_source(&self, name: &str, source: &str) -> DiagnosticResult<Program> {
        if let Some(budget) = &self.budget {
            budget.check_parser_entry()?;
        }
        let limits = self
            .budget
            .as_ref()
            .map(|budget| budget.limits().clone())
            .unwrap_or_default();
        Program::parse_with_budgets(
            name,
            source,
            limits.source_bytes,
            &limits.syntax,
            &limits.ast,
        )
        .map_err(|error| self.retain_limit(error))
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

    pub(crate) fn after_operation<T>(&self, result: DiagnosticResult<T>) -> DiagnosticResult<T> {
        match self.checkpoint() {
            Ok(()) => result,
            Err(stopped) => match result {
                Err(original) if original.code() == stopped.code() => Err(original),
                Err(original) => Err(stopped.while_handling(original)),
                Ok(_) => Err(stopped),
            },
        }
    }

    pub(crate) fn register_run_native(
        &mut self,
        signature: StatementSignature,
        callback: impl Fn(&[Literal], &RunEnvironment) -> LiteralResult + Send + Sync + 'static,
    ) -> DiagnosticResult<()> {
        if signature.origin() != StatementOrigin::Native {
            return Err(Diagnostic::new(BWErr::SignatureError(
                "Native registration requires a native signature".into(),
            ))
            .at(signature.header()));
        }
        self.insert_native(
            signature,
            Arc::new(move |values, context| {
                let environment = context.environment.as_ref().ok_or_else(|| {
                    BWErr::RunConfiguration("Native operation needs a configured run".into())
                })?;
                callback(values, environment)
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
            validate_value(value).map_err(|_| {
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
    }

    fn get_variable_binding(
        &self,
        name: &str,
        span: Option<&Span>,
    ) -> DiagnosticResult<&Arc<StoredValue>> {
        let mut index = Some(self.current);
        while let Some(frame_index) = index {
            let frame = &self.frames[frame_index];
            if let Some(value) = frame.variables.get(name) {
                return Ok(value);
            }
            index = frame.parent;
        }
        Err(self.detail_error(BWErr::VariableNotDefined, name, span, true))
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
    /// Callbacks return an owned finite value (including None) or a typed error.
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
            return Err(Diagnostic::new(BWErr::SignatureError(
                "Native registration requires a native signature".into(),
            ))
            .at(signature.header()));
        }
        self.insert_native(signature, Arc::new(move |values, _| callback(values)))
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
        self.check_statement_collision(signature.normalized(), signature.header())?;
        let registry = self
            .reserve_registry(RegistryPlan::signature(&signature))
            .map_err(|error| error.at(signature.header()))?;
        let metadata = Arc::new(signature);
        self.insert_statement(
            metadata.normalized(),
            metadata.header(),
            StmtType::Native {
                callback,
                builtin_log: false,
                metadata: Arc::clone(&metadata),
                _registry: registry,
            },
        )
    }

    fn insert_statement(
        &mut self,
        signature: &str,
        span: &Span,
        statement: StmtType,
    ) -> DiagnosticResult<()> {
        self.check_statement_collision(signature, span)?;
        let key = statement.registry().map_or_else(
            || Arc::from(signature),
            |reservation| Arc::clone(&reservation.key),
        );
        self.frames[self.current].statements.insert(key, statement);
        Ok(())
    }

    fn check_statement_collision(&self, signature: &str, span: &Span) -> DiagnosticResult<()> {
        if let Some((namespace, _)) = signature.split_once("::") {
            if let Some(original) = self.frames[self.current].namespaces.get(namespace) {
                return Err(imports::namespace_collision(
                    namespace,
                    &original.span,
                    span,
                ));
            }
        }
        let statements = &self.frames[self.current].statements;
        if let Some(original) = statements.get(signature) {
            let (origin, origin_span) = match original {
                StmtType::Native { metadata, .. } => (
                    metadata.header().source().name().to_owned(),
                    metadata.header(),
                ),
                StmtType::UserDefined { definition, .. } => {
                    (definition.span.location(), &definition.span)
                }
                StmtType::Imported { metadata, .. } => {
                    (metadata.header().location(), metadata.header())
                }
            };
            return Err(Diagnostic::new(BWErr::DuplicateStatement {
                signature: signature.into(),
                original: origin,
                duplicate: span.location(),
            })
            .at(span)
            .with_related("first definition", origin_span));
        }
        Ok(())
    }

    /// Fill vacant built-in slots, preserving existing registrations.
    pub fn init_statements(&mut self) {
        if !self.frames[self.current]
            .statements
            .contains_key("log|param|")
        {
            let signature = StatementSignature::native_at("<builtin Log>", "Log |value|")
                .expect("valid built-in header")
                .description("Write the value to stdout followed by a newline; return that value.")
                .documents_error(
                    super::diagnostic::DiagnosticCode::Output,
                    "The destination rejected output; some bytes may already be written.",
                )
                .expect("valid built-in error documentation");
            // The single fixed builtin is outside user registry admission. This
            // keeps infallible initialization usable with zero registry budgets.
            let metadata = Arc::new(signature);
            self.insert_statement(
                metadata.normalized(),
                metadata.header(),
                StmtType::Native {
                    callback: Arc::new(|values, _| log_param(values)),
                    builtin_log: true,
                    metadata: Arc::clone(&metadata),
                    _registry: None,
                },
            )
            .expect("the built-in Log signature is valid and vacant");
        }
    }

    fn with_call(
        &mut self,
        signature: &str,
        call_site: &Span,
        definition_site: Option<&Span>,
        body: impl FnOnce(&mut Self) -> TemporaryResult,
    ) -> TemporaryResult {
        self.check_call_depth()
            .map_err(|error| self.diagnostic(error, Some(call_site), false))?;
        let frame = self.retain_call(signature, call_site, definition_site)?;
        self.calls.push(frame);
        let result = body(self).map_err(|error| self.diagnostic(error, None, false));
        self.calls.pop();
        result
    }
}

type Callback = Arc<dyn Fn(&[Literal], &mut Context) -> LiteralResult + Send + Sync>;

fn log_param(values: &[Literal]) -> LiteralResult {
    let value = &values[0]; // Arity was checked before entering the callback.
    write_log(value, &mut io::stdout().lock())?;
    Ok(value.clone())
}

fn write_log(value: &Literal, output: &mut impl Write) -> Result<(), BWErr> {
    writeln!(output, "{value}").map_err(|error| BWErr::OutputError(error.to_string()))
}

fn invoke(call: &Call, context: &mut Context) -> TemporaryResult {
    invoke_inner(call, context).map_err(|error| context.diagnostic(error, Some(&call.span), false))
}

fn invoke_inner(call: &Call, context: &mut Context) -> TemporaryResult {
    let (definition, owner) = context.get_statement(&call.signature).ok_or_else(|| {
        context.detail_error(
            BWErr::StatementNotDefined,
            call.span.text(),
            Some(&call.span),
            false,
        )
    })?;
    let metadata = definition.metadata();
    let parameter_count = metadata.parameters().len();
    if parameter_count != call.arguments.len() {
        return Err(BWErr::ParameterMissingError(
            "The call does not match the definition's parameter count".into(),
        )
        .into());
    }
    context.check_call_depth()?;
    let mut argument_slots = context
        .budget
        .as_ref()
        .map(|budget| budget.reserve_argument_slots(parameter_count))
        .transpose()?;
    let arguments = call
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            if let Some(reservation) = &mut argument_slots {
                reservation.release_argument_slot();
            }
            let value = evaluate_expression(argument, context)?;
            validate_value(&value)
                .map_err(|error| Diagnostic::new(error).at_expression(&argument.span))?;
            metadata.validate_argument(index, &value, |message| {
                context.formatted_error(
                    BWErr::OperationIncompatibleError,
                    message,
                    Some(&argument.span),
                    true,
                )
            })?;
            Ok(value)
        })
        .collect::<DiagnosticResult<Vec<_>>>()?;
    invoke_resolved(call, definition, owner, arguments, context)
}

fn invoke_resolved(
    call: &Call,
    definition: StmtType,
    owner: usize,
    arguments: Vec<TemporaryValue>,
    context: &mut Context,
) -> TemporaryResult {
    let _depth = context
        .enter_evaluation()
        .map_err(|error| context.diagnostic(error, Some(&call.span), false))?;
    match definition {
        StmtType::Imported {
            module,
            exported,
            import_site,
            ..
        } => imports::invoke_imported(call, &module, &exported, arguments, &import_site, context),
        StmtType::Native {
            callback,
            metadata,
            builtin_log,
            ..
        } => context.with_call(&call.signature, &call.span, None, |context| {
            let arguments = TemporaryArguments::new(arguments);
            let known_result = if builtin_log {
                let size = context
                    .limits()
                    .values
                    .check(&arguments[0])
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                Some(context.temporary_reservation(size)?)
            } else {
                None
            };
            let result = catch_unwind(AssertUnwindSafe(|| callback(&arguments, context)))
                .map_err(|_| {
                    context.detail_error(
                        BWErr::NativePanic,
                        &call.signature,
                        Some(&call.span),
                        false,
                    )
                })
                .and_then(|result| {
                    result.map_err(|error| {
                        context.diagnostic(Diagnostic::new(error), Some(&call.span), false)
                    })
                });
            let result = context.after_operation(result.map(Owned::new))?;
            context.check_value(&result)?;
            validate_value(&result)?;
            metadata.validate_return(&result, |message| {
                context.formatted_error(
                    BWErr::OperationIncompatibleError,
                    message,
                    Some(&call.span),
                    false,
                )
            })?;
            match known_result {
                Some(reservation) => Ok(TemporaryValue::new(result.into_inner(), reservation)),
                None => context.temporary(result.into_inner()),
            }
        }),
        StmtType::UserDefined {
            definition,
            metadata,
            ..
        } => {
            let frame = Frame {
                variables: definition
                    .parameters
                    .iter()
                    .zip(arguments)
                    .map(|(parameter, value)| {
                        let (value, _reservation) = value.into_parts();
                        let value = context.store_value(value)?;
                        context
                            .retain_name(&parameter.text)
                            .map(|name| (name, value))
                    })
                    .collect::<DiagnosticResult<HashMap<_, _>>>()?,
                parent: Some(owner),
                ..Frame::default()
            };
            context.with_call(
                &call.signature,
                &call.span,
                Some(&definition.span),
                |context| {
                    context
                        .with_invocation(frame, |context| evaluate_block(&definition.body, context))
                        .and_then(|completion| {
                            let value = match completion {
                                Completion::Return(value) => value,
                                // Reject unconsumed controls at their invocation boundary.
                                completion => finish_script(completion)?,
                            };
                            metadata.validate_return(&value, |message| {
                                context.formatted_error(
                                    BWErr::OperationIncompatibleError,
                                    message,
                                    Some(&call.span),
                                    false,
                                )
                            })?;
                            Ok(value)
                        })
                },
            )
        }
    }
}

fn evaluate_expression(expression: &Expr, context: &mut Context) -> TemporaryResult {
    let _depth = context
        .enter_evaluation()
        .map_err(|error| context.diagnostic(error, Some(&expression.span), true))?;
    evaluate_expression_inner(expression, context)
        .and_then(|value| {
            context.check_value(&value)?;
            Ok(value)
        })
        .map_err(|error| context.diagnostic(error, Some(&expression.span), true))
}

fn evaluate_expression_inner(expression: &Expr, context: &mut Context) -> TemporaryResult {
    context.tick()?;
    #[cfg(test)]
    context
        .expression_visits
        .borrow_mut()
        .push(expression.span.text().to_owned());

    match &expression.kind {
        ExprKind::Integer(text) => text
            .parse::<i32>()
            .map(Literal::Int)
            .map_err(|error| Diagnostic::new(BWErr::ParsingIntegerError(error.to_string())))
            .and_then(|value| context.temporary(value)),
        ExprKind::Float(text) => {
            let value = text
                .parse::<f32>()
                .map_err(|error| BWErr::ParsingIntegerError(error.to_string()))?;
            context.temporary(finite_float(value, "Float literal")?)
        }
        ExprKind::Bool(value) => context.temporary(Literal::Bool(*value)),
        ExprKind::String(value) => context.temporary_string(value),
        ExprKind::Variable(name) => context.copy_temporary(
            &context
                .get_variable_binding(name, Some(&expression.span))?
                .value,
        ),
        ExprKind::Call(call) => invoke(call, context),
        ExprKind::Access { base, segments } => evaluate_access(base, segments, context),
        ExprKind::Array(elements) => {
            let limits = context.limits().values;
            let admit = |error| context.retain_limit(Diagnostic::new(error));
            let mut size = limits.container_header(elements.len()).map_err(admit)?;
            let mut planned = size;
            planned.nodes += elements.len();
            let reservation = context.temporary_reservation(planned)?;
            let mut result = TemporaryValue::new(
                Literal::Array(Vec::with_capacity(elements.len())),
                reservation,
            );
            for element in elements {
                result.release_child(super::value_limits::ValueSize {
                    nodes: 1,
                    ..Default::default()
                });
                let value = evaluate_expression(element, context)?;
                let child = limits
                    .check(&value)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                limits
                    .add_child(&mut size, child)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                let (value, reservation) = value.into_parts();
                let Literal::Array(values) = &mut result.value else {
                    unreachable!()
                };
                values.push(value);
                result.absorb(reservation);
            }
            Ok(result)
        }
        ExprKind::Map(entries) => {
            let limits = context.limits().values;
            let mut size = limits
                .container_header(0)
                .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
            let mut keys = HashSet::new();
            for (key, _) in entries {
                limits
                    .key_size(key.text.len())
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                if !keys.contains(key.text.as_str()) {
                    limits
                        .container_header(keys.len() + 1)
                        .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                    limits
                        .add_bytes(&mut size, key.text.len())
                        .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                    keys.insert(key.text.as_str());
                }
            }
            let mut planned = size;
            planned.nodes += keys.len();
            let reservation = context.temporary_reservation(planned)?;
            let mut result = TemporaryValue::new(Literal::Map(HashMap::new()), reservation);
            for (key, expression) in entries {
                let Literal::Map(values) = &result.value else {
                    unreachable!()
                };
                if !values.contains_key(&key.text) {
                    result.release_child(super::value_limits::ValueSize {
                        nodes: 1,
                        ..Default::default()
                    });
                }
                let value = evaluate_expression(expression, context)?;
                let child = limits
                    .check(&value)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                let Literal::Map(values) = &result.value else {
                    unreachable!()
                };
                let old_size = if let Some(previous) = values.get(&key.text) {
                    let old = limits
                        .check(previous)
                        .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                    size.nodes -= old.nodes;
                    size.payload_bytes -= old.payload_bytes;
                    // Keeping a previous maximum depth is safe within this map;
                    // the completed value is measured afresh by its caller.
                    Some(old)
                } else {
                    None
                };
                limits
                    .add_child(&mut size, child)
                    .map_err(|error| context.retain_limit(Diagnostic::new(error)))?;
                let (value, reservation) = value.into_parts();
                let Literal::Map(values) = &mut result.value else {
                    unreachable!()
                };
                if let Some(previous) = values.get_mut(&key.text) {
                    let old = std::mem::replace(previous, value);
                    drop(old);
                    result.release_child(old_size.expect("existing map child"));
                } else {
                    values.insert(key.text.clone(), value);
                }
                result.absorb(reservation);
            }
            Ok(result)
        }
        ExprKind::Unary {
            operator, operand, ..
        } => match (operator, &operand.kind) {
            (UnaryOp::Negate, ExprKind::Integer(text)) => {
                context.tick()?;
                // Convert the signed atom together: MIN's positive magnitude is not i32.
                // Compound operands still evaluate normally before checked negation.
                #[cfg(test)]
                context
                    .expression_visits
                    .borrow_mut()
                    .push(operand.span.text().to_owned());
                format!("-{text}")
                    .parse::<i32>()
                    .map(Literal::Int)
                    .map_err(|error| Diagnostic::new(BWErr::ParsingIntegerError(error.to_string())))
                    .and_then(|value| context.temporary(value))
            }
            _ => {
                let operand = evaluate_expression(operand, context)?;
                context.temporary_unary(*operator, operand, &expression.span)
            }
        },
        ExprKind::Binary {
            operator,
            left,
            right,
            ..
        } => {
            let left = evaluate_expression(left, context)?;
            if matches!(operator, BinaryOp::And | BinaryOp::Or) {
                let Literal::Bool(value) = &*left else {
                    let name = if *operator == BinaryOp::And {
                        "and"
                    } else {
                        "or"
                    };
                    return Err(context.formatted_error(
                        BWErr::OperationIncompatibleError,
                        format_args!("The left operand of `{name}` must be a boolean"),
                        Some(&expression.span),
                        true,
                    ));
                };
                if (*operator == BinaryOp::And && !value) || (*operator == BinaryOp::Or && *value) {
                    return Ok(left);
                }
            }
            let right = evaluate_expression(right, context)?;
            context.temporary_binary(*operator, left, right, &expression.span)
        }
    }
}

fn evaluate_access(
    base: &Expr,
    segments: &[AccessSegment],
    context: &mut Context,
) -> TemporaryResult {
    // Retain an immutable snapshot across effectful index calls without copying
    // the whole variable container. Only the selected result is copied.
    let binding;
    let temporary;
    let mut value = if let ExprKind::Variable(name) = &base.kind {
        #[cfg(test)]
        context
            .expression_visits
            .borrow_mut()
            .push(base.span.text().to_owned());
        binding = Arc::clone(context.get_variable_binding(name, Some(&base.span))?);
        &binding.value
    } else {
        temporary = evaluate_expression(base, context)?;
        &temporary
    };
    let error = |context: &Context, segment: &AccessSegment, reason: std::fmt::Arguments<'_>| {
        context.access_error(base, segments, segment, reason)
    };
    for segment in segments {
        // Evaluate this key before checking its receiver/type; do not evaluate
        // any later key until this lookup succeeds.
        let key = match segment {
            AccessSegment::Literal(_) => None,
            AccessSegment::Computed { index, .. } => Some(evaluate_expression(index, context)?),
        };
        value = match value {
            Literal::Map(values) => {
                let name = match (segment, key.as_deref()) {
                    (AccessSegment::Literal(name), _) => &name.text,
                    (_, Some(Literal::String(name))) => name,
                    _ => {
                        return Err(error(
                            context,
                            segment,
                            format_args!("map key must be a string"),
                        ))
                    }
                };
                values.get(name).ok_or_else(|| {
                    error(context, segment, format_args!("map key does not exist"))
                })?
            }
            Literal::Array(values) => {
                let index = match (segment, key.as_deref()) {
                    (AccessSegment::Literal(name), _) => {
                        if name.text.is_empty()
                            || !name.text.bytes().all(|byte| byte.is_ascii_digit())
                        {
                            return Err(error(
                                context,
                                segment,
                                format_args!("array index must contain ASCII decimal digits"),
                            ));
                        }
                        name.text.parse::<usize>().ok()
                    }
                    (_, Some(Literal::Int(index))) if *index >= 0 => usize::try_from(*index).ok(),
                    _ => {
                        return Err(error(
                            context,
                            segment,
                            format_args!("array index must be a nonnegative integer"),
                        ))
                    }
                };
                index.and_then(|index| values.get(index)).ok_or_else(|| {
                    error(
                        context,
                        segment,
                        format_args!("array index is out of bounds for length {}", values.len()),
                    )
                })?
            }
            _ => {
                return Err(error(
                    context,
                    segment,
                    format_args!("value is neither a map nor an array"),
                ))
            }
        };
    }
    context.copy_temporary(value)
}

fn evaluate_block(block: &Block, context: &mut Context) -> CompletionResult {
    for statement in &block.statements {
        match evaluate_statement(statement, context)? {
            Completion::Normal(_) => (),
            control => return Ok(control),
        }
    }
    Ok(Completion::Normal(context.temporary(Literal::None)?))
}

fn evaluate_for(
    binding: &str,
    iterable: &Expr,
    body: &Block,
    context: &mut Context,
) -> CompletionResult {
    let iterable_value = evaluate_expression(iterable, context)?;
    if !matches!(&*iterable_value, Literal::Array(_)) {
        return Err(Diagnostic::new(BWErr::OperationIncompatibleError(
            "For requires an array to iterate over".into(),
        ))
        .at_expression(&iterable.span));
    }
    let (iterable_value, _iterable_reservation) = iterable_value.into_parts();
    let Literal::Array(values) = iterable_value else {
        unreachable!()
    };
    let previous = context.frames[context.current]
        .variables
        .remove_entry(binding);
    let mut iterator_name = previous.as_ref().map(|(name, _)| name.clone());
    let result = (|| {
        for value in values {
            context.tick()?;
            let stored = context.store_value(value)?;
            if iterator_name.is_none() {
                iterator_name = Some(context.retain_name(binding)?);
            }
            context.frames[context.current]
                .variables
                .insert(iterator_name.as_ref().unwrap().clone(), stored);
            match evaluate_block(body, context)? {
                Completion::Normal(_) | Completion::Continue => (),
                Completion::Break => break,
                returned @ Completion::Return(_) => return Ok(returned),
            }
        }
        Ok(Completion::Normal(context.temporary(Literal::None)?))
    })();
    if let Some((name, value)) = previous {
        context.frames[context.current]
            .variables
            .insert(name, value);
    } else {
        context.frames[context.current].variables.remove(binding);
    }
    result
}

fn evaluate_while(condition: &Expr, body: &Block, context: &mut Context) -> CompletionResult {
    loop {
        let value = evaluate_expression(condition, context)?;
        let Literal::Bool(should_loop) = &*value else {
            return Err(Diagnostic::new(BWErr::OperationIncompatibleError(
                "While requires a boolean condition".into(),
            ))
            .at_expression(&condition.span));
        };
        let should_loop = *should_loop;
        drop(value);
        if !should_loop {
            break;
        }
        match evaluate_block(body, context)? {
            Completion::Normal(_) | Completion::Continue => (),
            Completion::Break => break,
            returned @ Completion::Return(_) => return Ok(returned),
        }
    }
    Ok(Completion::Normal(context.temporary(Literal::None)?))
}

fn evaluate_handler(
    binding: Option<&Name>,
    handler: &Block,
    original: Diagnostic,
    context: &mut Context,
) -> CompletionResult {
    let owner = context.current;
    let original = context.retain_handler(original)?;
    let previous = if let Some(name) = binding {
        let installed = (|| {
            context.checkpoint()?;
            let limits = context.limits();
            let limits = limits.diagnostic_values.intersect(&limits.values);
            let size = original
                .value
                .value_size_with_limits(&limits)
                .map_err(|error| context.retain_limit(error))?;
            let reservation = context.temporary_reservation(size)?;
            let value = TemporaryValue::new(original.value.to_value(), reservation);
            let (value, _reservation) = value.into_parts();
            context.set_variable(&name.text, value)
        })();
        match installed {
            Ok(previous) => previous,
            Err(error) => {
                return Err(error
                    .at(&name.span)
                    .while_handling(StoredDiagnostic::into_diagnostic(original)))
            }
        }
    } else {
        None
    };
    let handler_name = binding.map(|name| {
        context.frames[owner]
            .variables
            .get_key_value(name.text.as_str())
            .expect("installed handler binding")
            .0
            .clone()
    });
    context.handlers.push(HandledError {
        invocation: owner,
        diagnostic: original.clone(),
    });
    let result = evaluate_block(handler, context);
    context.handlers.pop();
    let result =
        result.map_err(|error| error.while_handling(StoredDiagnostic::into_diagnostic(original)));
    if let Some(name) = binding {
        // Reservation counters never participate in key equality or hashing.
        #[allow(clippy::mutable_key_type)]
        let variables = &mut context.frames[owner].variables;
        if let Some(value) = previous {
            variables.insert(handler_name.expect("handler name"), value);
        } else {
            variables.remove(name.text.as_str());
        }
    }
    result
}

fn evaluate_statement(statement: &Statement, context: &mut Context) -> CompletionResult {
    let _depth = context
        .enter_evaluation()
        .map_err(|error| context.diagnostic(error, Some(&statement.span), false))?;
    evaluate_statement_inner(statement, context)
        .map_err(|error| context.diagnostic(error, Some(&statement.span), false))
}

// Declaration admission runs only for definitions; its scratch state must not
// increase every active import/control frame's stack requirements.
#[inline(never)]
fn evaluate_definition(definition: &Arc<Definition>, context: &mut Context) -> CompletionResult {
    context.check_statement_collision(&definition.signature, &definition.span)?;
    let reservation = context
        .budget
        .as_ref()
        .map(|budget| budget.reserve_definition(definition))
        .transpose()
        .map_err(|error| context.retain_limit(error))?;
    let registry = context.reserve_registry(RegistryPlan::definition(definition))?;
    context.insert_statement(
        &definition.signature,
        &definition.span,
        StmtType::UserDefined {
            definition: Arc::clone(definition),
            metadata: Arc::new(definition.signature_metadata()),
            _reservation: reservation,
            _registry: registry,
        },
    )?;
    Ok(Completion::Normal(context.temporary(Literal::None)?))
}

fn evaluate_statement_inner(statement: &Statement, context: &mut Context) -> CompletionResult {
    context.tick()?;
    match &statement.kind {
        StatementKind::Assign { name, value } => {
            let value = match value {
                AssignmentValue::Expression(expression) => {
                    evaluate_expression(expression, context)?
                }
                AssignmentValue::Call(call) => invoke(call, context)?,
            };
            // Reserve the stored copy before cloning its potentially large payload.
            let reservation = context.reserve_value(&value)?;
            let name = context.variable_key(&name.text, context.current)?;
            let stored = Arc::new(StoredValue::new(value.clone(), reservation));
            context.frames[context.current]
                .variables
                .insert(name, stored);
            Ok(Completion::Normal(value))
        }
        StatementKind::Define(definition) => evaluate_definition(definition, context),
        StatementKind::Invoke(call) => invoke(call, context).map(Completion::Normal),
        StatementKind::Import {
            path,
            path_span,
            namespace,
        } => imports::evaluate_import(path, path_span, namespace, &statement.span, context)
            .and_then(|value| context.temporary(value))
            .map(Completion::Normal),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let value = evaluate_expression(condition, context)?;
            let Literal::Bool(condition) = &*value else {
                return Err(Diagnostic::new(BWErr::OperationIncompatibleError(
                    "If requires a boolean condition".into(),
                ))
                .at_expression(&condition.span));
            };
            let condition = *condition;
            drop(value);
            if condition {
                evaluate_block(then_branch, context)
            } else {
                match else_branch {
                    Some(ElseBranch::Block(block)) => evaluate_block(block, context),
                    Some(ElseBranch::If(statement)) => evaluate_statement(statement, context),
                    None => Ok(Completion::Normal(context.temporary(Literal::None)?)),
                }
            }
        }
        StatementKind::For {
            binding,
            iterable,
            body,
        } => evaluate_for(&binding.text, iterable, body, context),
        StatementKind::While { condition, body } => evaluate_while(condition, body, context),
        StatementKind::Try {
            body,
            binding,
            handler,
        } => match evaluate_block(body, context) {
            Ok(value) => Ok(value),
            Err(original) => {
                let original = context
                    .after_operation::<Literal>(Err(original))
                    .unwrap_err();
                if context.checkpoint().is_err() {
                    Err(original)
                } else {
                    evaluate_handler(binding.as_ref(), handler, original, context)
                }
            }
        },
        StatementKind::Return(expression) => {
            let value = match expression {
                Some(expression) => evaluate_expression(expression, context)?,
                None => context.temporary(Literal::None)?,
            };
            Ok(Completion::Return(value))
        }
        StatementKind::Break => Ok(Completion::Break),
        StatementKind::Continue => Ok(Completion::Continue),
        StatementKind::Rethrow => {
            match context
                .handlers
                .last()
                .filter(|handler| handler.invocation == context.current)
            {
                Some(handler) => Err(handler
                    .diagnostic
                    .value
                    .clone()
                    .with_related("rethrow", &statement.span)),
                None => Err(BWErr::ControlFlowError(
                    "Rethrow requires an enclosing Catch in the same invocation".into(),
                )
                .into()),
            }
        }
    }
}

// Retain runtime boundary guards even though public entry points validate placement.
fn finish_script(completion: Completion) -> TemporaryResult {
    match completion {
        Completion::Normal(value) => Ok(value),
        Completion::Return(_) => {
            Err(BWErr::ControlFlowError("Return requires a custom-statement body".into()).into())
        }
        Completion::Break => Err(BWErr::ControlFlowError(
            "Break requires an enclosing loop in the same invocation".into(),
        )
        .into()),
        Completion::Continue => Err(BWErr::ControlFlowError(
            "Continue requires an enclosing loop in the same invocation".into(),
        )
        .into()),
    }
}

/// Evaluate an already parsed, owned statement at script level in this context.
///
/// Definitions retain their syntax tree and source spans after the program is dropped.
/// The entire statement is validated before execution; custom calls consume their returns.
pub fn execute_statement(statement: &Statement, context: &mut Context) -> LiteralResult {
    execute_statement_detailed(statement, context).map_err(Diagnostic::into_error)
}

/// Execute one script-level statement with source locations and entered-call frames.
pub fn execute_statement_detailed(statement: &Statement, context: &mut Context) -> RuntimeResult {
    let result = (|| {
        context.checkpoint()?;
        let statements = std::slice::from_ref(statement);
        let limits = context.limits();
        super::ast_limits::check_statements(statements, &limits.ast, limits.source_bytes)
            .map_err(|error| context.retain_limit(error))?;
        ast::validate_control_script_detailed(statements)?;
        finish_script(evaluate_statement(statement, context)?).map(TemporaryValue::into_inner)
    })();
    result.map_err(|error| context.diagnostic(error, None, false))
}

/// Validate the complete program, then execute without parsing or rebuilding it.
pub fn evaluate_program(program: &Program, context: &mut Context) -> LiteralResult {
    evaluate_program_detailed(program, context).map_err(Diagnostic::into_error)
}

/// Validate and execute a program while preserving structured diagnostic causes.
pub fn evaluate_program_detailed(program: &Program, context: &mut Context) -> RuntimeResult {
    let result = (|| {
        context.checkpoint()?;
        let limits = context.limits();
        program
            .validate_with_limits(&limits.ast, limits.source_bytes)
            .map_err(|error| context.retain_limit(error))?;
        let mut result = None;
        for statement in &program.statements {
            // A replaced script result is unobservable once the next statement starts.
            drop(result.take());
            result = Some(finish_script(evaluate_statement(statement, context)?)?);
        }
        result
            .map_or_else(|| context.temporary(Literal::None), Ok)
            .map(TemporaryValue::into_inner)
    })();
    result.map_err(|error| context.diagnostic(error, None, false))
}

/// Compatibility entry point for callers that already hold a Pest pair.
///
/// This lowers the pair once. Prefer `Program::parse` and `evaluate_program` to
/// share one owned source allocation across an entire script.
pub fn botwork(pair: Pair<Rule>, context: &mut Context) -> LiteralResult {
    botwork_detailed(pair, context).map_err(Diagnostic::into_error)
}

/// Parser-pair compatibility with detailed execution errors.
pub fn botwork_detailed(pair: Pair<Rule>, context: &mut Context) -> RuntimeResult {
    let result = (|| {
        context.checkpoint()?;
        let node = ast::from_pair(pair)?;
        let limits = context.limits();
        super::ast_limits::check_node(&node, &limits.ast, limits.source_bytes)
            .map_err(|error| context.retain_limit(error))?;
        match node {
            Node::Statement(statement) => execute_statement_detailed(&statement, context),
            Node::Expression(expression) => {
                evaluate_expression(&expression, context).map(TemporaryValue::into_inner)
            }
            Node::Block(block) => {
                ast::validate_control_script_detailed(&block.statements)?;
                finish_script(evaluate_block(&block, context)?).map(TemporaryValue::into_inner)
            }
            Node::None => context
                .temporary(Literal::None)
                .map(TemporaryValue::into_inner),
        }
    })();
    result.map_err(|error| context.diagnostic(error, None, false))
}
