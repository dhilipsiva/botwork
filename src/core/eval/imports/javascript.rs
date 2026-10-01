//! JavaScript modules (decision D7). `Import |"helpers.mjs"| As |js|` asks Node
//! for the statements the module exports, then publishes each as an isolated
//! operation: every call runs in a Node process of its own over the worker
//! protocol, so a stop ends the process, and module state lasts one call.
use super::*;
use crate::core::{
    signature::StatementSignature,
    worker::{protocol::WorkerProtocol, WorkerCommand, WorkerLimits, WorkerOutcome, WorkerPool},
};
use std::{collections::BTreeMap, ffi::OsString};

/// The Node side, run with `node --input-type=module -e`.
const HOST: &str = include_str!("javascript_host.mjs");
/// The most of Node's error output a loading failure keeps, from its end.
const ERROR_BYTES: usize = 2048;

/// A loaded JavaScript file: its statements, unqualified, each with the
/// command that calls it.
pub(in crate::core::eval) struct Module {
    statements: Vec<(StatementSignature, WorkerCommand)>,
}

/// The extensions of the files this adapter loads.
pub(super) fn handles(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|value| value.to_str()),
        Some("js" | "mjs" | "cjs")
    )
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
    let pool = context.modules.javascript_pool()?;
    let statements = module.statements.iter().map(|(signature, command)| {
        let (pool, command) = (pool.clone(), command.clone());
        (signature, move |qualified| {
            NativeOperation::isolated(qualified, pool, command, WorkerProtocol::default())
        })
    });
    publish_operations(statements, namespace, normalized, import_site, context)
}

/// Node, from `PATH`.
fn node() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["node.exe"]
    } else {
        &["node"]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_absolute() && path.is_file())
}

fn command(
    node: &Path,
    file: &Path,
    arguments: &[&str],
    environment: &BTreeMap<OsString, OsString>,
) -> WorkerCommand {
    let mut words: Vec<OsString> = ["--input-type=module", "-e", HOST, "--"]
        .into_iter()
        .map(OsString::from)
        .collect();
    words.push(arguments[0].into());
    words.push(file.as_os_str().to_owned());
    words.extend(arguments[1..].iter().map(OsString::from));
    WorkerCommand {
        executable: node.to_owned(),
        arguments: words,
        directory: file.parent().map(Path::to_owned).unwrap_or_default(),
        environment: environment.clone(),
    }
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
    if let Some(module) = context.modules.javascript.get(&canonical) {
        return Ok(Arc::clone(module));
    }
    let node = node().ok_or_else(|| {
        failure(
            context,
            format_args!("`{path}` is a JavaScript module, which needs Node.js on PATH"),
        )
    })?;
    // Calls see the run's environment, as process statements do.
    let environment = context
        .environment
        .as_ref()
        .map(|environment| environment.variables().clone())
        .unwrap_or_default();
    let pool = context
        .modules
        .javascript_pool()
        .map_err(|error| related(context, error))?;
    let control = context
        .budget
        .as_ref()
        .map(|budget| budget.control().clone())
        .unwrap_or_default();
    let listing = pool
        .start(
            command(&node, &canonical, &["list"], &environment),
            Vec::new(),
            control.child(None),
        )
        .map_err(|error| related(context, error))?
        .wait()
        .await;
    context.checkpoint()?;
    let file = canonical.display().to_string();
    if listing.outcome != WorkerOutcome::Succeeded {
        let reason = String::from_utf8_lossy(&listing.stderr);
        let reason = reason.trim();
        let mut start = reason.len().saturating_sub(ERROR_BYTES);
        while !reason.is_char_boundary(start) {
            start += 1;
        }
        let reason = match (&reason[start..], listing.diagnostic) {
            ("", Some(diagnostic)) => diagnostic.to_string(),
            (text, _) => text.to_owned(),
        };
        return Err(failure(context, format_args!("{file}: Node: {reason}")));
    }
    let headers: Vec<(String, usize)> = serde_json::from_slice(&listing.stdout)
        .map_err(|error| failure(context, format_args!("{file}: {error}")))?;
    let origin = format!(
        "<javascript {}>",
        canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file)
    );
    let mut statements = Vec::with_capacity(headers.len());
    for (index, (header, declared)) in headers.into_iter().enumerate() {
        let signature = StatementSignature::native_at(&origin, &header)
            .map_err(|error| related(context, error))?;
        let count = signature.parameters().len();
        // A function's length counts the parameters before any default or rest
        // one; more than the header passes would leave some undefined.
        if declared > count {
            return Err(failure(
                context,
                format_args!(
                    "{file}: the function for `{header}` takes {declared} argument(s), but the header passes {count}"
                ),
            ));
        }
        let index = index.to_string();
        statements.push((
            signature,
            command(&node, &canonical, &["call", &index], &environment),
        ));
    }
    let module = Arc::new(Module { statements });
    context
        .modules
        .javascript
        .insert(canonical, Arc::clone(&module));
    Ok(module)
}

/// One pool serves a run's JavaScript calls; each call is one worker.
pub(super) fn pool() -> DiagnosticResult<WorkerPool> {
    WorkerPool::new(WorkerLimits::default())
}
