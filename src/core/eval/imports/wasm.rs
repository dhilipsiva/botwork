//! WebAssembly modules (decision D8). `Import |"tools.wasm"| As |tools|`
//! compiles a component exporting the `statements` interface of
//! `wit/botwork.wit` and publishes each statement it lists. Every call runs in
//! an instance of its own, with WASI Preview 2's clocks and randomness and
//! nothing else: no files, network, or environment. Fuel and a memory cap bound
//! each call, and a stop interrupts it through Wasmtime's epochs.
use super::*;
use crate::core::{run::WasmLimits, signature::StatementSignature, value_limits::ValueLimits};
use std::{num::NonZeroUsize, sync::OnceLock};
use wasmtime::{
    component::{Linker, ResourceTable},
    Config, Engine, Store, UpdateDeadline,
};
use wasmtime_wasi::{p2::pipe::MemoryOutputPipe, WasiCtx, WasiCtxView, WasiView};

mod bindings {
    wasmtime::component::bindgen!({ path: "wit/botwork.wit", world: "module" });
}
use bindings::{
    botwork::statements::types::{Failure, FailureKind, Node},
    ModulePre,
};

/// Calls of one statement that may run at once, each on a blocking thread.
const CAPACITY: usize = 4;
/// The most of a trap's stderr, and of its backtrace, a diagnostic keeps.
const TRACE_BYTES: usize = 2048;
/// The most table elements one instance may grow its tables to, in all.
const TABLE_ELEMENTS: usize = 1 << 20;

/// A module compiled and linked, ready to instantiate, with its headers.
type Compiled = (ModulePre<State>, Vec<String>);

/// A compiled module: its statements, unqualified, each with its index.
pub(in crate::core::eval) struct Module {
    pre: ModulePre<State>,
    statements: Vec<(StatementSignature, u32)>,
}

/// What a call's store holds: WASI, the limiter, and the call's control,
/// which the epoch callback checks.
struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limiter: Limiter,
    control: OperationControl,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Caps the total size of an instance's memories and tables, and remembers
/// refusing.
struct Limiter {
    memory_bytes: usize,
    memory_used: usize,
    table_elements: usize,
    refused: Option<(&'static str, u64)>,
}

impl Limiter {
    /// Admit growing one memory or table from `current` to `desired`, within
    /// `maximum` for all of them together.
    fn grow(used: &mut usize, current: usize, desired: usize, maximum: usize) -> Option<usize> {
        let total = used.saturating_sub(current).saturating_add(desired);
        (total <= maximum).then(|| std::mem::replace(used, total))
    }
}

impl wasmtime::ResourceLimiter for Limiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if Self::grow(&mut self.memory_used, current, desired, self.memory_bytes).is_none() {
            self.refused = Some(("WASM memory bytes", self.memory_bytes as u64));
            return Err(wasmtime::format_err!("the memory limit refused growth"));
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if Self::grow(&mut self.table_elements, current, desired, TABLE_ELEMENTS).is_none() {
            self.refused = Some(("WASM table elements", TABLE_ELEMENTS as u64));
            return Err(wasmtime::format_err!("the table limit refused growth"));
        }
        Ok(true)
    }
}

pub(super) async fn evaluate_import(
    path: &str,
    span: &Span,
    namespace: &Name,
    normalized: &str,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    let module = load(path, span, import_site, context).await?;
    let limits = context.limits();
    let limits = Arc::new((limits.values, limits.wasm));
    let statements = module.statements.iter().map(|(signature, index)| {
        let (module, index, limits) = (Arc::clone(&module), *index, Arc::clone(&limits));
        (signature, move |qualified| {
            NativeOperation::blocking(
                qualified,
                NonZeroUsize::new(CAPACITY).expect("nonzero capacity"),
                move |arguments, control| {
                    call(&module, index, arguments, control, &limits.0, &limits.1)
                },
            )
        })
    });
    publish_operations(statements, namespace, normalized, import_site, context)
}

/// The engine every module compiles for, with fuel and epoch interruption.
fn engine() -> Result<&'static Engine, String> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            config.consume_fuel(true).epoch_interruption(true);
            Engine::new(&config).map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// WASI Preview 2, which every instance links against; each store's context
/// decides what it grants.
fn linker(engine: &Engine) -> Result<&'static Linker<State>, String> {
    static LINKER: OnceLock<Result<Linker<State>, String>> = OnceLock::new();
    LINKER
        .get_or_init(|| {
            let mut linker = Linker::new(engine);
            wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
                .map(|()| linker)
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// The output a call wrote, kept to diagnose a trap.
struct Output {
    stdout: MemoryOutputPipe,
    stderr: MemoryOutputPipe,
}

/// A store for one call: no files, network, environment, or arguments; stdin
/// closed; stdout and stderr captured up to the limit; the host's clocks and
/// randomness. A stop of `control` interrupts the instance at its next epoch
/// check.
fn store(
    engine: &Engine,
    limits: &WasmLimits,
    control: OperationControl,
) -> (Store<State>, Output) {
    let output = Output {
        stdout: MemoryOutputPipe::new(limits.output_bytes),
        stderr: MemoryOutputPipe::new(limits.output_bytes),
    };
    let wasi = WasiCtx::builder()
        .stdout(output.stdout.clone())
        .stderr(output.stderr.clone())
        // Sockets are off by default; say so, so that new defaults cannot
        // open them.
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false)
        .build();
    let state = State {
        wasi,
        table: ResourceTable::new(),
        limiter: Limiter {
            memory_bytes: limits.memory_bytes,
            memory_used: 0,
            table_elements: 0,
            refused: None,
        },
        control,
    };
    let mut store = Store::new(engine, state);
    store.limiter(|state| &mut state.limiter);
    // Fuel is enabled on the engine, so setting it cannot fail.
    let _ = store.set_fuel(limits.fuel);
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(|store| {
        Ok(if store.data().control.checkpoint().is_err() {
            UpdateDeadline::Interrupt
        } else {
            // Another call's stop advanced the epoch.
            UpdateDeadline::Continue(1)
        })
    });
    (store, output)
}

/// Advance the engine's epoch when `control` stops, which interrupts every
/// running instance at its next check; each continues unless its own call
/// stopped. The returned task is aborted once the call returns.
fn watch(engine: &Engine, control: &OperationControl) -> Option<tokio::task::JoinHandle<()>> {
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    let (engine, control) = (engine.clone(), control.clone());
    Some(runtime.spawn(async move {
        control.stopped().await;
        engine.increment_epoch();
    }))
}

/// Run `work` with the store's epoch watched, so a stop interrupts it.
fn watched<T>(
    engine: &Engine,
    store: &mut Store<State>,
    work: impl FnOnce(&mut Store<State>) -> wasmtime::Result<T>,
) -> wasmtime::Result<T> {
    let control = store.data().control.clone();
    let watcher = watch(engine, &control);
    // A stop before the watcher started advanced no epoch.
    let result = if control.checkpoint().is_err() {
        Err(wasmtime::format_err!("stopped before starting"))
    } else {
        work(store)
    };
    if let Some(watcher) = watcher {
        watcher.abort();
    }
    result
}

async fn load(
    path: &str,
    span: &Span,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Arc<Module>> {
    let failure = |context: &Context, reason: std::fmt::Arguments<'_>| {
        context.import_error(BWErr::ImportRead, reason, span, import_site)
    };
    let related = |context: &Context, error: Diagnostic| {
        RuntimeDiagnostic::from(error).with_related_in(
            "imported here",
            import_site,
            context.budget.as_ref(),
            Some((span, false)),
            context.calls.iter().map(|record| &record.frame),
        )
    };
    let canonical = resolve(path, span, import_site, context).await?;
    if let Some(module) = context.modules.wasm.get(&canonical) {
        return Ok(Arc::clone(module));
    }
    let limits = context.limits().wasm;
    let file = canonical.display().to_string();
    let bytes = read_module_bytes(&canonical, limits.module_bytes, context)
        .await
        .map_err(|error| match error {
            SourceFailure::Io(error) => failure(context, format_args!("{file}: {error}")),
            SourceFailure::Diagnostic(error) => related(context, error),
        })?;
    // Compiling, linking, and listing the headers are blocking work: they run
    // on a worker, like other blocking jobs.
    let work = move |control: &OperationControl| -> Result<Compiled, SourceFailure> {
        let other = |reason: String| SourceFailure::Io(std::io::Error::other(reason));
        let engine = engine().map_err(other)?;
        let component = wasmtime::component::Component::new(engine, &bytes)
            .map_err(|error| other(format!("not a WebAssembly component: {}", root(&error))))?;
        let pre = linker(engine)
            .map_err(other)?
            .instantiate_pre(&component)
            .and_then(ModulePre::new)
            .map_err(|error| other(root(&error)))?;
        let (mut store, output) = store(engine, &limits, control.clone());
        let headers = watched(engine, &mut store, |store| {
            pre.instantiate(&mut *store)?
                .botwork_statements_statements()
                .call_headers(store)
        });
        control.checkpoint().map_err(SourceFailure::Diagnostic)?;
        let headers = headers.map_err(|error| {
            other(match trapped(&error, &store, &output, &limits) {
                Ok(limit) => limit.to_string(),
                Err(reason) => reason,
            })
        })?;
        Ok((pre, headers))
    };
    let control = context
        .budget
        .as_ref()
        .map(|budget| budget.control().clone())
        .unwrap_or_default();
    let (pre, headers) = if context.asynchronous {
        crate::core::run::blocking_io::run(control, work).await
    } else {
        work(&control)
    }
    .map_err(|error| match error {
        SourceFailure::Io(error) => failure(context, format_args!("{file}: {error}")),
        SourceFailure::Diagnostic(error) => related(context, error),
    })?;
    let origin = format!(
        "<wasm {}>",
        canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file)
    );
    let mut statements = Vec::with_capacity(headers.len());
    for (index, header) in headers.iter().enumerate() {
        let signature = StatementSignature::native_at(&origin, header)
            .map_err(|error| related(context, error))?;
        let index = u32::try_from(index)
            .map_err(|_| failure(context, format_args!("{file}: too many statements")))?;
        statements.push((signature, index));
    }
    let module = Arc::new(Module { pre, statements });
    context.modules.wasm.insert(canonical, Arc::clone(&module));
    Ok(module)
}

/// Call a statement on this blocking worker thread, in an instance of its own.
fn call(
    module: &Module,
    index: u32,
    arguments: Vec<Literal>,
    control: OperationControl,
    values: &ValueLimits,
    limits: &WasmLimits,
) -> DiagnosticResult<Literal> {
    let engine = engine().map_err(|reason| Diagnostic::new(BWErr::NativeError(reason)))?;
    let arguments: Vec<Vec<Node>> = arguments
        .iter()
        .map(|value| {
            let mut nodes = Vec::new();
            encode(value, &mut nodes);
            nodes
        })
        .collect();
    let (mut store, output) = store(engine, limits, control.clone());
    let outcome = watched(engine, &mut store, |store| {
        module
            .pre
            .instantiate(&mut *store)?
            .botwork_statements_statements()
            .call_call(store, index, &arguments)
    });
    match outcome {
        Ok(Ok(nodes)) => decode(nodes, values).map_err(|reason| {
            Diagnostic::new(BWErr::NativeError(format!(
                "The WASM statement returned {reason}"
            )))
        }),
        Ok(Err(Failure { kind, message })) => Err(Diagnostic::new(match kind {
            FailureKind::Assertion if message.is_empty() => {
                BWErr::AssertionFailed("a WASM assertion failed".into())
            }
            FailureKind::Assertion => BWErr::AssertionFailed(message),
            FailureKind::Error if message.is_empty() => {
                BWErr::NativeError("The WASM statement failed".into())
            }
            FailureKind::Error => BWErr::NativeError(message),
        })),
        // The operation reports the stop that interrupted the call.
        Err(_) if control.checkpoint().is_err() => Ok(Literal::None),
        Err(error) => Err(match trapped(&error, &store, &output, limits) {
            Ok(limit) => limit,
            Err(reason) => Diagnostic::new(BWErr::NativeError(reason)),
        }),
    }
}

/// Why a call failed: Ok with a resource limit it reached, or Err with a
/// description of its trap, the end of its stderr, and its backtrace.
fn trapped(
    error: &wasmtime::Error,
    store: &Store<State>,
    output: &Output,
    limits: &WasmLimits,
) -> Result<Diagnostic, String> {
    let limit = |resource: &'static str, limit: u64| {
        Diagnostic::new(BWErr::ResourceLimit { resource, limit })
    };
    if error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel) {
        return Ok(limit("WASM fuel", limits.fuel));
    }
    if let Some((resource, maximum)) = store.data().limiter.refused {
        return Ok(limit(resource, maximum));
    }
    let full = |pipe: &MemoryOutputPipe| pipe.contents().len() >= limits.output_bytes;
    if full(&output.stdout) {
        return Ok(limit("WASM stdout bytes", limits.output_bytes as u64));
    }
    if full(&output.stderr) {
        return Ok(limit("WASM stderr bytes", limits.output_bytes as u64));
    }
    let mut text = format!("WebAssembly trap: {}", root(error));
    let stderr = output.stderr.contents();
    let stderr = String::from_utf8_lossy(&stderr);
    let stderr = stderr.trim_end();
    if !stderr.is_empty() {
        let mut start = stderr.len().saturating_sub(TRACE_BYTES);
        while !stderr.is_char_boundary(start) {
            start += 1;
        }
        text.push('\n');
        if start > 0 {
            text.push_str("…\n");
        }
        text.push_str(&stderr[start..]);
    }
    if let Some(backtrace) = error.downcast_ref::<wasmtime::WasmBacktrace>() {
        let backtrace = backtrace.to_string();
        let mut end = backtrace.len().min(TRACE_BYTES);
        while !backtrace.is_char_boundary(end) {
            end -= 1;
        }
        text.push('\n');
        text.push_str(backtrace[..end].trim_end());
        if end < backtrace.len() {
            text.push_str("\n…");
        }
    }
    Err(text)
}

/// The innermost cause of a Wasmtime error, without its backtrace.
fn root(error: &wasmtime::Error) -> String {
    error.root_cause().to_string()
}

/// Lay `value` out as nodes, depth first, and return its node's index.
fn encode(value: &Literal, nodes: &mut Vec<Node>) -> u32 {
    let index = nodes.len();
    nodes.push(Node::None);
    let node = match value {
        Literal::None => Node::None,
        Literal::Int(number) => Node::Int(*number),
        Literal::Float(number) => Node::Float(*number),
        Literal::Bool(flag) => Node::Boolean(*flag),
        Literal::String(text) => Node::Text(text.clone()),
        Literal::Array(items) => {
            Node::Array(items.iter().map(|item| encode(item, nodes)).collect())
        }
        Literal::Map(entries) => {
            // In key order, so a module sees the same layout every time.
            let mut entries: Vec<_> = entries.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            Node::Map(
                entries
                    .into_iter()
                    .map(|(key, item)| (key.clone(), encode(item, nodes)))
                    .collect(),
            )
        }
    };
    nodes[index] = node;
    index as u32
}

/// A returned value, within the run's value limits. Its nodes must form a
/// tree: each node but the first is named once, by an earlier node.
fn decode(nodes: Vec<Node>, limits: &ValueLimits) -> Result<Literal, String> {
    if nodes.is_empty() {
        return Err("a value with no nodes".into());
    }
    if nodes.len() > limits.nodes {
        return Err(format!("a value of more than {} nodes", limits.nodes));
    }
    let mut depths: Vec<Option<usize>> = vec![None; nodes.len()];
    depths[0] = Some(0);
    for (index, node) in nodes.iter().enumerate() {
        let depth = depths[index]
            .ok_or_else(|| format!("a value whose node {index} is not in its tree"))?;
        let children: Vec<u32> = match node {
            Node::Array(items) => items.clone(),
            Node::Map(entries) => entries.iter().map(|(_, item)| *item).collect(),
            _ => continue,
        };
        for child in children {
            let child = child as usize;
            if child <= index || child >= nodes.len() {
                return Err(format!(
                    "a value whose node {index} names node {child}, which is not after it"
                ));
            }
            if depths[child].replace(depth + 1).is_some() {
                return Err(format!("a value that names node {child} twice"));
            }
            // Checked before building, so nothing recurses deeper.
            if depth + 1 > limits.depth {
                return Err(format!(
                    "a value nested more than {} levels deep",
                    limits.depth
                ));
            }
        }
    }
    // Children come after their parents, so building from the last node
    // finishes every child before its parent needs it.
    let mut built: Vec<Option<Literal>> =
        std::iter::repeat_with(|| None).take(nodes.len()).collect();
    let take = |built: &mut Vec<Option<Literal>>, index: u32| {
        built[index as usize]
            .take()
            .expect("children are built first")
    };
    for (index, node) in nodes.into_iter().enumerate().rev() {
        let literal = match node {
            Node::None => Literal::None,
            Node::Int(number) => Literal::Int(number),
            Node::Float(number) if number.is_finite() => Literal::Float(number),
            Node::Float(number) => {
                return Err(format!("the float {number}, which is not finite"));
            }
            Node::Boolean(flag) => Literal::Bool(flag),
            Node::Text(text) => Literal::String(text),
            Node::Array(items) => Literal::Array(
                items
                    .into_iter()
                    .map(|item| take(&mut built, item))
                    .collect(),
            ),
            Node::Map(entries) => {
                let mut map = HashMap::with_capacity(entries.len().min(limits.entries));
                for (key, item) in entries {
                    let value = take(&mut built, item);
                    if map.contains_key(&key) {
                        return Err(format!("a map with the key {key:?} twice"));
                    }
                    map.insert(key, value);
                }
                Literal::Map(map)
            }
        };
        built[index] = Some(literal);
    }
    let literal = built[0].take().expect("the root is built");
    limits
        .check(&literal)
        .map(|_| literal)
        .map_err(|error| format!("a value beyond the value limits: {error}"))
}
