//! Python modules (decision D9). `Import |"helpers.py"| As |h|` runs the file
//! in a module object of its own, then publishes every function marked with
//! `@botwork.statement("…")` as a statement in the namespace. One interpreter
//! serves the process, as CPython requires; each run gets its own module
//! objects, so module globals never pass between runs.
use super::*;
use crate::core::{operation::OperationControl, signature::StatementSignature};
use pyo3::{
    exceptions::PyAssertionError,
    ffi,
    prelude::*,
    types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple},
};
use std::{
    ffi::CString,
    num::NonZeroUsize,
    os::raw::c_long,
    sync::{Mutex, OnceLock},
};

/// The module scripts import as `botwork`.
const BOTWORK: &str = r#"
"""Statements for Botwork scripts, written in Python."""


class Stopped(BaseException):
    """Raised in a statement Botwork has stopped: cancelled, or past its deadline."""


def statement(header):
    """Make the decorated function the Botwork statement `header`, such as
    "Greet |name|". Botwork passes its parameters in order."""
    if not isinstance(header, str):
        raise TypeError('a statement header is a str, such as "Greet |name|"')

    def mark(function):
        if not callable(function):
            raise TypeError("@botwork.statement marks a function")
        function.__botwork_statement__ = header
        return function

    return mark
"#;

/// Calls one statement may have in flight; the interpreter lock runs one
/// Python thread at a time in any case.
const CAPACITY: usize = 4;
/// The most of a traceback a diagnostic keeps, from its end.
const TRACEBACK_BYTES: usize = 2048;

/// A loaded Python file: its statements, unqualified, with their functions.
pub(in crate::core::eval) struct Module {
    statements: Vec<(StatementSignature, Arc<Py<PyAny>>)>,
}

/// A marked function, with its parsed header and whether it accepts the
/// header's parameters.
struct Prepared {
    name: String,
    header: String,
    function: Py<PyAny>,
    signature: DiagnosticResult<StatementSignature>,
    accepts: bool,
}

/// Start the interpreter once and install the `botwork` module.
fn interpreter() -> Result<(), String> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            Python::initialize();
            Python::attach(|py| {
                // `threading` takes the thread that first imports it as the
                // main thread; make that the thread that started the
                // interpreter, as CPython's own checks assume. Otherwise
                // asyncio on Windows believes a worker thread is the main
                // thread and calls `signal.set_wakeup_fd`, which refuses.
                py.import("threading")
                    .map_err(|error| format!("Python could not import threading: {error}"))?;
                let source = CString::new(BOTWORK).expect("no NUL in the module source");
                PyModule::from_code(py, &source, c"botwork", c"botwork")
                    .map(|_| ())
                    .map_err(|error| format!("Python could not define the botwork module: {error}"))
            })
        })
        .clone()
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
    publish(&module, namespace, normalized, import_site, context)
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
    if path.contains("://") {
        return Err(failure(
            context,
            format_args!("`{path}` must name a local .py file"),
        ));
    }
    let canonical = resolve(path, span, import_site, context).await?;
    if let Some(module) = context.modules.python.get(&canonical) {
        return Ok(Arc::clone(module));
    }
    let source = read_module_source(&canonical, context)
        .await
        .map_err(|error| match error {
            SourceFailure::Io(error) => {
                failure(context, format_args!("{}: {error}", canonical.display()))
            }
            SourceFailure::Diagnostic(error) => related(context, error),
        })?;
    let file = canonical
        .to_str()
        .ok_or_else(|| failure(context, format_args!("Module paths must be valid UTF-8")))?
        .to_owned();
    let directory = canonical
        .parent()
        .and_then(Path::to_str)
        .unwrap_or_default()
        .to_owned();
    // Headers are parsed on their own, so their locations name the file
    // without suggesting a line in it, as other native headers do.
    let origin = format!(
        "<python {}>",
        canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file)
    );
    // Starting the interpreter and running the file's top level are blocking
    // work: they run on a worker, like other blocking jobs, and a stop raises
    // `botwork.Stopped` in them.
    let work = {
        let file = file.clone();
        move |control: &OperationControl| -> Result<Vec<Prepared>, SourceFailure> {
            let other = |reason: String| SourceFailure::Io(std::io::Error::other(reason));
            interpreter().map_err(other)?;
            let prepared = interruptible(control, |py| {
                prepare(py, &source, &file, &directory, &origin)
            });
            control.checkpoint().map_err(SourceFailure::Diagnostic)?;
            prepared.map_err(other)
        }
    };
    let control = context
        .budget
        .as_ref()
        .map(|budget| budget.control().clone())
        .unwrap_or_default();
    let prepared = if context.asynchronous {
        crate::core::run::blocking_io::run(control, work).await
    } else {
        work(&control)
    }
    .map_err(|error| match error {
        SourceFailure::Io(error) => failure(context, format_args!("{file}: {error}")),
        SourceFailure::Diagnostic(error) => related(context, error),
    })?;
    let mut statements = Vec::with_capacity(prepared.len());
    for Prepared {
        name,
        header,
        function,
        signature,
        accepts,
    } in prepared
    {
        let signature = signature.map_err(|error| related(context, error))?;
        if !accepts {
            let count = signature.parameters().len();
            return Err(failure(
                context,
                format_args!(
                    "{file}: `{name}` does not take the {count} argument(s) of `{header}`"
                ),
            ));
        }
        statements.push((signature, Arc::new(function)));
    }
    let module = Arc::new(Module { statements });
    context
        .modules
        .python
        .insert(canonical, Arc::clone(&module));
    Ok(module)
}

/// Run `source` in a new module object, outside `sys.modules`, and return the
/// marked functions in definition order, with their names and headers.
fn execute(
    py: Python<'_>,
    source: &str,
    file: &str,
    directory: &str,
) -> Result<Vec<(String, String, Py<PyAny>)>, String> {
    let describe = |error: PyErr| describe(py, &error);
    let module = PyModule::new(py, "botwork_module").map_err(describe)?;
    module.setattr("__file__", file).map_err(describe)?;
    let builtins = py.import("builtins").map_err(describe)?;
    let code = builtins
        .getattr("compile")
        .and_then(|compile| compile.call1((source, file, "exec")))
        .map_err(describe)?;
    // The file can import modules beside it while it loads.
    let path = py
        .import("sys")
        .and_then(|sys| sys.getattr("path"))
        .map_err(describe)?;
    path.call_method1("insert", (0, directory))
        .map_err(describe)?;
    let ran = builtins
        .getattr("exec")
        .and_then(|exec| exec.call1((code, module.dict())));
    let restored = path.call_method1("remove", (directory,));
    ran.map_err(describe)?;
    restored.map_err(describe)?;
    let mut marked = Vec::new();
    for (name, value) in module.dict().iter() {
        let Ok(header) = value.getattr("__botwork_statement__") else {
            continue;
        };
        let header: String = header.extract().map_err(describe)?;
        let name: String = name.extract().unwrap_or_default();
        marked.push((name, header, value.unbind()));
    }
    Ok(marked)
}

/// Run the file and check each marked function against its header.
fn prepare(
    py: Python<'_>,
    source: &str,
    file: &str,
    directory: &str,
    origin: &str,
) -> Result<Vec<Prepared>, String> {
    Ok(execute(py, source, file, directory)?
        .into_iter()
        .map(|(name, header, function)| {
            let signature = StatementSignature::native_at(origin, &header);
            let accepts = signature
                .as_ref()
                .is_ok_and(|signature| accepts(py, &function, signature.parameters().len()));
            Prepared {
                name,
                header,
                function,
                signature,
                accepts,
            }
        })
        .collect())
}

/// Whether `function` can be called with `count` positional arguments.
fn accepts(py: Python<'_>, function: &Py<PyAny>, count: usize) -> bool {
    py.import("inspect")
        .and_then(|inspect| inspect.getattr("signature")?.call1((function.bind(py),)))
        .and_then(|signature| {
            signature.call_method1("bind", PyTuple::new(py, (0..count).map(|_| py.None()))?)
        })
        .is_ok()
}

fn publish(
    module: &Arc<Module>,
    namespace: &Name,
    normalized: &str,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    if let Some(budget) = &context.budget {
        let mut bytes = normalized.len();
        for (signature, _) in &module.statements {
            bytes = signature
                .qualified_bytes(&namespace.text, normalized)
                .and_then(|size| bytes.checked_add(size))
                .ok_or_else(|| {
                    context.runtime_diagnostic(
                        budget.import_limit(ImportResource::MetadataBytes).into(),
                        Some(import_site),
                        false,
                    )
                })?;
        }
        budget
            .charge_imports(&[
                (ImportResource::Bindings, module.statements.len() + 1),
                (ImportResource::MetadataBytes, bytes),
            ])
            .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
    }
    let namespace_registry = context
        .reserve_registry(RegistryPlan::namespace(normalized, import_site))
        .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
    let namespace_key = namespace_registry.as_ref().map_or_else(
        || Arc::from(normalized),
        |reservation| Arc::clone(&reservation.key),
    );
    // Admit every statement before publishing any.
    let mut operations = Vec::with_capacity(module.statements.len());
    for (signature, function) in &module.statements {
        let registry = context
            .reserve_registry(RegistryPlan::qualified(
                signature,
                &namespace.text,
                normalized,
                import_site,
            ))
            .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
        let qualified = signature.qualified(&namespace.text, normalized);
        let key = registry.as_ref().map_or_else(
            || Arc::from(qualified.normalized()),
            |reservation| Arc::clone(&reservation.key),
        );
        let function = Arc::clone(function);
        let operation = NativeOperation::blocking(
            qualified,
            NonZeroUsize::new(CAPACITY).expect("nonzero capacity"),
            move |arguments, control| call(&function, arguments, control),
        )
        .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
        operations.push((key, operation, registry));
    }
    let frame = &mut context.frames[context.current];
    for (key, operation, registry) in operations {
        frame.statements.insert(
            key,
            StmtType::Operation {
                operation,
                builtin: false,
                _registry: registry,
            },
        );
    }
    frame.namespaces.insert(
        namespace_key,
        StoredNamespace {
            span: import_site.clone(),
            _registry: namespace_registry,
        },
    );
    Ok(Literal::None)
}

/// Call a statement's function on this blocking worker thread.
fn call(
    function: &Py<PyAny>,
    arguments: Vec<Literal>,
    control: OperationControl,
) -> DiagnosticResult<Literal> {
    interruptible(&control, |py| {
        let values = arguments
            .iter()
            .map(|value| into_python(py, value))
            .collect::<PyResult<Vec<_>>>()
            .map_err(|error| failure(py, &error))?;
        let outcome = PyTuple::new(py, values).and_then(|values| {
            let value = function.bind(py).call1(values)?;
            // An `async def` statement runs its coroutine to completion here.
            if py
                .import("inspect")?
                .getattr("iscoroutine")?
                .call1((&value,))?
                .is_truthy()?
            {
                py.import("asyncio")?.getattr("run")?.call1((value,))
            } else {
                Ok(value)
            }
        });
        match outcome {
            Ok(value) => from_python(&value).map_err(|reason| {
                Diagnostic::new(BWErr::NativeError(format!(
                    "The Python statement returned {reason}"
                )))
            }),
            // The operation reports the stop that interrupted the call; a
            // value returned now is discarded.
            Err(error) if stopped(py, &error) => Ok(Literal::None),
            Err(error) => Err(failure(py, &error)),
        }
    })
}

/// Run `work` with the interpreter lock on this thread. A stop of `control`
/// meanwhile raises `botwork.Stopped` in the thread; code blocked in native
/// code sees it only when it returns to Python.
fn interruptible<T>(control: &OperationControl, work: impl FnOnce(Python<'_>) -> T) -> T {
    // The thread running `work`, set and cleared while holding the
    // interpreter lock, so an interruption never reaches later work.
    let running: Arc<Mutex<Option<c_long>>> = Arc::new(Mutex::new(None));
    let watcher = tokio::runtime::Handle::try_current().ok().map(|runtime| {
        let control = control.clone();
        let running = Arc::clone(&running);
        runtime.spawn(async move {
            control.stopped().await;
            let _ = tokio::task::spawn_blocking(move || interrupt(&running)).await;
        })
    });
    let result = Python::attach(|py| {
        let thread: Option<c_long> = py
            .import("threading")
            .and_then(|threading| threading.getattr("get_ident")?.call0()?.extract())
            .ok();
        *running.lock().unwrap_or_else(|error| error.into_inner()) = thread;
        let value = work(py);
        *running.lock().unwrap_or_else(|error| error.into_inner()) = None;
        if let Some(thread) = thread {
            // SAFETY: clears an interruption that arrived after `work` returned.
            unsafe { ffi::PyThreadState_SetAsyncExc(thread, std::ptr::null_mut()) };
        }
        value
    });
    if let Some(watcher) = watcher {
        watcher.abort();
    }
    result
}

/// Raise `botwork.Stopped` in the thread running a call, if one is.
fn interrupt(running: &Mutex<Option<c_long>>) {
    Python::attach(|py| {
        let Some(thread) = *running.lock().unwrap_or_else(|error| error.into_inner()) else {
            return;
        };
        if let Ok(stopped) = py
            .import("botwork")
            .and_then(|botwork| botwork.getattr("Stopped"))
        {
            // SAFETY: the interpreter lock is held and the class lives in the
            // botwork module for the rest of the process.
            unsafe { ffi::PyThreadState_SetAsyncExc(thread, stopped.as_ptr()) };
        }
    });
}

fn stopped(py: Python<'_>, error: &PyErr) -> bool {
    py.import("botwork")
        .and_then(|botwork| botwork.getattr("Stopped"))
        .is_ok_and(|class| error.get_type(py).is(&class))
}

/// A diagnostic for a Python exception: BW9001 for an `AssertionError`, BW4002
/// for any other, with the exception's type, message, and traceback's end.
fn failure(py: Python<'_>, error: &PyErr) -> Diagnostic {
    let message = error
        .value(py)
        .str()
        .map(|text| text.to_string())
        .unwrap_or_default();
    if error.is_instance_of::<PyAssertionError>(py) {
        let reason = if message.is_empty() {
            "a Python assertion failed".to_owned()
        } else {
            message
        };
        return Diagnostic::new(BWErr::AssertionFailed(reason));
    }
    Diagnostic::new(BWErr::NativeError(describe(py, error)))
}

fn describe(py: Python<'_>, error: &PyErr) -> String {
    let kind = error
        .get_type(py)
        .qualname()
        .map(|name| name.to_string())
        .unwrap_or_else(|_| "exception".into());
    let message = error
        .value(py)
        .str()
        .map(|text| text.to_string())
        .unwrap_or_default();
    let mut text = if message.is_empty() {
        format!("Python {kind}")
    } else {
        format!("Python {kind}: {message}")
    };
    if let Some(traceback) = error
        .traceback(py)
        .and_then(|traceback| traceback.format().ok())
    {
        let mut start = traceback.len().saturating_sub(TRACEBACK_BYTES);
        while !traceback.is_char_boundary(start) {
            start += 1;
        }
        text.push('\n');
        if start > 0 {
            text.push_str("…\n");
        }
        text.push_str(traceback[start..].trim_end());
    }
    text
}

fn into_python<'py>(py: Python<'py>, value: &Literal) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Literal::None => py.None().into_bound(py),
        Literal::Int(number) => number.into_pyobject(py)?.into_any(),
        Literal::Float(number) => f64::from(*number).into_pyobject(py)?.into_any(),
        Literal::Bool(flag) => PyBool::new(py, *flag).to_owned().into_any(),
        Literal::String(text) => PyString::new(py, text).into_any(),
        Literal::Array(items) => PyList::new(
            py,
            items
                .iter()
                .map(|item| into_python(py, item))
                .collect::<PyResult<Vec<_>>>()?,
        )?
        .into_any(),
        Literal::Map(entries) => {
            let map = PyDict::new(py);
            for (key, item) in entries {
                map.set_item(key, into_python(py, item)?)?;
            }
            map.into_any()
        }
    })
}

/// Convert a returned value, within the default value limits. Every value
/// without an exact Botwork equivalent is refused, never coerced.
fn from_python(value: &Bound<'_, PyAny>) -> Result<Literal, String> {
    let limits = crate::core::value_limits::ValueLimits::default();
    let mut nodes = 0;
    let literal = convert(value, 0, &mut nodes, &limits)?;
    limits
        .check(&literal)
        .map(|_| literal)
        .map_err(|error| format!("a value beyond the value limits: {error}"))
}

fn convert(
    value: &Bound<'_, PyAny>,
    depth: usize,
    nodes: &mut usize,
    limits: &crate::core::value_limits::ValueLimits,
) -> Result<Literal, String> {
    *nodes += 1;
    if *nodes > limits.nodes || depth > limits.depth {
        return Err("a value beyond the value limits".into());
    }
    if value.is_none() {
        return Ok(Literal::None);
    }
    // bool is a subclass of int, so it is checked first.
    if let Ok(flag) = value.cast::<PyBool>() {
        return Ok(Literal::Bool(flag.is_true()));
    }
    if let Ok(number) = value.cast::<PyInt>() {
        return number
            .extract::<i32>()
            .map(Literal::Int)
            .map_err(|_| format!("the int {number}, outside Botwork's 32-bit integers"));
    }
    if let Ok(number) = value.cast::<PyFloat>() {
        let wide = number.value();
        let narrow = wide as f32;
        // Spelled as Python writes it, for the statement's author.
        let written = number
            .repr()
            .map(|text| text.to_string())
            .unwrap_or_else(|_| wide.to_string());
        return if narrow.is_finite() {
            Ok(Literal::Float(narrow))
        } else {
            Err(format!(
                "the float {written}, which no finite 32-bit float holds"
            ))
        };
    }
    if let Ok(text) = value.cast::<PyString>() {
        let text = text
            .to_str()
            .map_err(|_| "a str that is not valid Unicode".to_owned())?;
        if text.len() > limits.string_bytes {
            return Err("a value beyond the value limits".into());
        }
        return Ok(Literal::String(text.to_owned()));
    }
    if let Ok(items) = value.cast::<PyList>() {
        return items
            .iter()
            .map(|item| convert(&item, depth + 1, nodes, limits))
            .collect::<Result<_, _>>()
            .map(Literal::Array);
    }
    if let Ok(items) = value.cast::<PyTuple>() {
        return items
            .iter()
            .map(|item| convert(&item, depth + 1, nodes, limits))
            .collect::<Result<_, _>>()
            .map(Literal::Array);
    }
    if let Ok(entries) = value.cast::<PyDict>() {
        let mut map = HashMap::with_capacity(entries.len().min(limits.entries));
        for (key, item) in entries.iter() {
            let key = key
                .cast::<PyString>()
                .map_err(|_| "a dict with a key that is not a str".to_owned())?
                .to_str()
                .map_err(|_| "a dict key that is not valid Unicode".to_owned())?
                .to_owned();
            map.insert(key, convert(&item, depth + 1, nodes, limits)?);
        }
        return Ok(Literal::Map(map));
    }
    let kind = value
        .get_type()
        .qualname()
        .map(|name| name.to_string())
        .unwrap_or_else(|_| "object".into());
    Err(format!("a {kind}, which has no Botwork equivalent"))
}
