use super::*;
use crate::core::run::{ImportResource, SnapshotSize};
use std::path::Path;

struct ImportChain<'a> {
    loading: &'a [PathBuf],
    repeated: &'a Path,
}

impl std::fmt::Display for ImportChain<'_> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, path) in self
            .loading
            .iter()
            .map(PathBuf::as_path)
            .chain(std::iter::once(self.repeated))
            .enumerate()
        {
            if index != 0 {
                output.write_str(" -> ")?;
            }
            write!(output, "{}", path.display())?;
        }
        Ok(())
    }
}

mod javascript;
pub(crate) mod playwright;
#[cfg(feature = "python")]
mod python;
#[cfg(feature = "wasm")]
mod wasm;
pub(crate) mod webdriver;

#[derive(Default)]
pub(super) struct ModuleCache {
    loaded: HashMap<PathBuf, Arc<LoadedModule>>,
    resolved: HashMap<PathBuf, PathBuf>,
    /// Python files this run has loaded, each in a module object of its own.
    #[cfg(feature = "python")]
    python: HashMap<PathBuf, Arc<python::Module>>,
    /// JavaScript files this run has loaded, and the pool their calls run in.
    javascript: HashMap<PathBuf, Arc<javascript::Module>>,
    javascript_pool: Option<crate::core::worker::WorkerPool>,
    /// WebAssembly files this run has compiled.
    #[cfg(feature = "wasm")]
    wasm: HashMap<PathBuf, Arc<wasm::Module>>,
    /// The package project of this run's `@` imports, found at the first.
    project: Option<Arc<crate::core::packages::Project>>,
    /// The browser sessions this run opened, made at its first WebDriver import.
    webdriver: Option<Arc<webdriver::Sessions>>,
    /// The Playwright host this run's statements share, made at its first import.
    playwright: Option<Arc<playwright::Host>>,
}

impl Clone for ModuleCache {
    fn clone(&self) -> Self {
        Self {
            loaded: self
                .loaded
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value)))
                .collect(),
            resolved: self
                .resolved
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            #[cfg(feature = "python")]
            python: self
                .python
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value)))
                .collect(),
            javascript: self
                .javascript
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value)))
                .collect(),
            javascript_pool: self.javascript_pool.clone(),
            #[cfg(feature = "wasm")]
            wasm: self
                .wasm
                .iter()
                .map(|(key, value)| (key.clone(), Arc::clone(value)))
                .collect(),
            project: self.project.clone(),
            webdriver: self.webdriver.clone(),
            playwright: self.playwright.clone(),
        }
    }
}

impl ModuleCache {
    pub(super) fn snapshot_size(&self, size: &mut SnapshotSize) {
        size.entries(self.loaded.len());
        size.entries(self.resolved.len());
        for path in self.loaded.keys() {
            size.path(path);
        }
        #[cfg(feature = "python")]
        {
            size.entries(self.python.len());
            for path in self.python.keys() {
                size.path(path);
            }
        }
        size.entries(self.javascript.len());
        for path in self.javascript.keys() {
            size.path(path);
        }
        #[cfg(feature = "wasm")]
        {
            size.entries(self.wasm.len());
            for path in self.wasm.keys() {
                size.path(path);
            }
        }
        if let Some(project) = &self.project {
            size.path(project.root());
        }
        for (requested, canonical) in &self.resolved {
            size.path(requested);
            size.path(canonical);
        }
    }

    /// The sessions a run's WebDriver statements share, made at the first import.
    fn webdriver_sessions(&mut self) -> Arc<webdriver::Sessions> {
        Arc::clone(self.webdriver.get_or_insert_with(Default::default))
    }

    /// The Playwright host a run's statements share, made at the first import;
    /// it starts its Node process at the first command.
    fn playwright_host(
        &mut self,
        directory: &Path,
        variables: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    ) -> Arc<playwright::Host> {
        Arc::clone(self.playwright.get_or_insert_with(|| {
            Arc::new(playwright::Host::new(directory.to_owned(), variables))
        }))
    }

    /// The pool a run's JavaScript calls share, made at the first import.
    fn javascript_pool(&mut self) -> DiagnosticResult<crate::core::worker::WorkerPool> {
        if self.javascript_pool.is_none() {
            self.javascript_pool = Some(javascript::pool()?);
        }
        Ok(self.javascript_pool.clone().expect("just made"))
    }
}

#[cfg(test)]
mod tests;

pub(super) struct LoadedModule {
    frame: Frame,
}

pub(super) fn namespace_collision(
    context: &Context,
    namespace: &str,
    original: &Span,
    duplicate: &Span,
) -> RuntimeDiagnostic {
    context.duplicate_error(
        |[namespace, original, duplicate]| BWErr::DuplicateNamespace {
            namespace,
            original,
            duplicate,
        },
        namespace,
        (original, false),
        duplicate,
        "first namespace occupant",
    )
}

pub(super) async fn evaluate_import(
    path: &str,
    path_span: &Span,
    namespace: &Name,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    let normalized = ast::normalize_sentence(&namespace.text);
    let frame = &context.frames[context.current];
    if let Some(original) = frame.namespaces.get(normalized.as_str()) {
        return Err(namespace_collision(
            context,
            &normalized,
            &original.span,
            &namespace.span,
        ));
    }
    let prefix = format!("{normalized}::");
    if let Some((_, statement)) = frame
        .statements
        .iter()
        .filter(|(name, _)| name.starts_with(&prefix))
        .min_by_key(|(name, _)| *name)
    {
        return Err(namespace_collision(
            context,
            &normalized,
            statement.metadata().header(),
            &namespace.span,
        ));
    }
    if path == webdriver::PATH {
        return webdriver::evaluate_import(namespace, &normalized, import_site, context).await;
    }
    if path == playwright::PATH {
        return playwright::evaluate_import(namespace, &normalized, import_site, context).await;
    }
    if javascript::handles(path) {
        return javascript::evaluate_import(
            path,
            path_span,
            namespace,
            &normalized,
            import_site,
            context,
        )
        .await;
    }
    if Path::new(path).extension().and_then(|value| value.to_str()) == Some("wasm") {
        #[cfg(feature = "wasm")]
        return wasm::evaluate_import(
            path,
            path_span,
            namespace,
            &normalized,
            import_site,
            context,
        )
        .await;
        #[cfg(not(feature = "wasm"))]
        return Err(context.import_error(
            BWErr::ImportRead,
            format_args!(
                "`{path}` is a WebAssembly module, which needs a Botwork build with the `wasm` feature"
            ),
            path_span,
            import_site,
        ));
    }
    if Path::new(path).extension().and_then(|value| value.to_str()) == Some("py") {
        #[cfg(feature = "python")]
        return python::evaluate_import(
            path,
            path_span,
            namespace,
            &normalized,
            import_site,
            context,
        )
        .await;
        #[cfg(not(feature = "python"))]
        return Err(context.import_error(
            BWErr::ImportRead,
            format_args!(
                "`{path}` is a Python module, which needs a Botwork build with the `python` feature"
            ),
            path_span,
            import_site,
        ));
    }
    let module = load_module(path, path_span, import_site, context).await?;
    publish_namespace(&module, namespace, &normalized, import_site, context)
}

// Keep publication scratch storage off the recursive module-loading stack.
#[inline(never)]
fn publish_namespace(
    module: &Arc<LoadedModule>,
    namespace: &Name,
    normalized: &str,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Literal> {
    admit_namespace(module, namespace, normalized, context)
        .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
    let namespace_registry = context
        .reserve_registry(RegistryPlan::namespace(normalized, import_site))
        .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
    let namespace_key = namespace_registry.as_ref().map_or_else(
        || Arc::from(normalized),
        |reservation| Arc::clone(&reservation.key),
    );
    let mut exports = Vec::new();
    // Admit every wrapper before copying metadata or publishing the namespace.
    let mut declarations: Vec<_> = module.frame.statements.iter().collect();
    declarations.sort_unstable_by_key(|(left, _)| *left);
    for (exported, statement) in declarations {
        if matches!(
            statement,
            StmtType::Native { .. } | StmtType::Operation { .. }
        ) {
            continue;
        }
        let registry = context
            .reserve_registry(RegistryPlan::qualified(
                statement.metadata(),
                &namespace.text,
                normalized,
                import_site,
            ))
            .map_err(|error| context.runtime_diagnostic(error.into(), Some(import_site), false))?;
        exports.push((Arc::clone(exported), statement, registry));
    }
    let frame = &mut context.frames[context.current];
    for (exported, statement, registry) in exports {
        let metadata = Arc::new(statement.metadata().qualified(&namespace.text, normalized));
        let key = registry.as_ref().map_or_else(
            || Arc::from(metadata.normalized()),
            |reservation| Arc::clone(&reservation.key),
        );
        frame.statements.insert(
            key,
            StmtType::Imported {
                module: Arc::clone(module),
                exported,
                metadata,
                import_site: import_site.clone(),
                _registry: registry,
            },
        );
    }
    frame.dependency_depth = frame
        .dependency_depth
        .max(module.frame.dependency_depth + 1);
    frame.namespaces.insert(
        namespace_key,
        StoredNamespace {
            span: import_site.clone(),
            _registry: namespace_registry,
        },
    );
    Ok(Literal::None)
}

/// Publish native operations as a namespace's statements, as the adapters for
/// other languages do: each takes its unqualified signature and makes the
/// operation for the qualified one. Every statement is admitted before any is
/// published.
fn publish_operations<'a, F>(
    statements: impl ExactSizeIterator<Item = (&'a StatementSignature, F)>,
    namespace: &Name,
    normalized: &str,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Literal>
where
    F: FnOnce(StatementSignature) -> DiagnosticResult<NativeOperation>,
{
    let statements: Vec<_> = statements.collect();
    if let Some(budget) = &context.budget {
        let mut bytes = normalized.len();
        for (signature, _) in &statements {
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
                (ImportResource::Bindings, statements.len() + 1),
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
    let mut operations = Vec::with_capacity(statements.len());
    for (signature, make) in statements {
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
        let operation = make(qualified)
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

/// The first `name`, with one of `suffixes`, in the directories of the run's
/// `PATH`. On Windows the variable's name is matched in any case, as Windows
/// matches it: its environment usually spells it `Path`.
pub(in crate::core::eval) fn on_path(
    variables: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    name: &str,
    suffixes: &[&str],
) -> Option<PathBuf> {
    let (_, path) = variables.iter().find(|(key, _)| {
        if cfg!(windows) {
            key.to_str()
                .is_some_and(|key| key.eq_ignore_ascii_case("PATH"))
        } else {
            key.as_os_str() == "PATH"
        }
    })?;
    std::env::split_paths(path)
        .flat_map(|directory| {
            suffixes
                .iter()
                .map(move |suffix| directory.join(format!("{name}{suffix}")))
        })
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
}

fn check_dependency_depth(context: &Context, dependency: usize) -> DiagnosticResult<()> {
    if let Some(budget) = &context.budget {
        let maximum = budget.limits().imports.dependency_depth;
        if context
            .loading
            .len()
            .checked_add(dependency)
            .and_then(|depth| depth.checked_add(1))
            .is_none_or(|depth| depth > maximum)
        {
            return Err(budget.limit("module dependency depth", maximum as u64));
        }
    }
    Ok(())
}

fn admit_namespace(
    module: &LoadedModule,
    namespace: &Name,
    normalized: &str,
    context: &Context,
) -> DiagnosticResult<()> {
    check_dependency_depth(context, module.frame.dependency_depth)?;
    if let Some(budget) = &context.budget {
        let mut bindings = 1usize;
        let mut bytes = normalized.len();
        for statement in module.frame.statements.values().filter(|statement| {
            !matches!(
                statement,
                StmtType::Native { .. } | StmtType::Operation { .. }
            )
        }) {
            bindings = bindings
                .checked_add(1)
                .ok_or_else(|| budget.import_limit(ImportResource::Bindings))?;
            bytes = statement
                .metadata()
                .qualified_bytes(&namespace.text, normalized)
                .and_then(|size| bytes.checked_add(size))
                .ok_or_else(|| budget.import_limit(ImportResource::MetadataBytes))?;
        }
        budget.charge_imports(&[
            (ImportResource::Bindings, bindings),
            (ImportResource::MetadataBytes, bytes),
        ])?;
    }
    Ok(())
}

/// Read a binary module of at most `maximum` bytes, counted as one load. Its
/// bytes are not source text, so they leave the source byte budget alone.
#[cfg(feature = "wasm")]
async fn read_module_bytes(
    path: &Path,
    maximum: usize,
    context: &mut Context,
) -> Result<Vec<u8>, SourceFailure> {
    if let Some(budget) = context.budget.as_ref().map(RunBudget::shared) {
        budget
            .charge_imports(&[
                (ImportResource::Loads, 1),
                (ImportResource::MetadataBytes, path.as_os_str().len()),
            ])
            .map_err(SourceFailure::Diagnostic)?;
    }
    context.read_source_bytes(path, maximum).await
}

async fn read_module_source(path: &Path, context: &mut Context) -> Result<String, SourceFailure> {
    let Some(budget) = context.budget.as_ref().map(RunBudget::shared) else {
        return context.read_source_async(path).await;
    };
    budget
        .charge_imports(&[
            (ImportResource::Loads, 1),
            (ImportResource::MetadataBytes, path.as_os_str().len()),
        ])
        .map_err(SourceFailure::Diagnostic)?;
    let remaining = budget.import_remaining(ImportResource::SourceBytes);
    let bytes = context
        .read_source_bytes(path, remaining.min(budget.limits().source_bytes))
        .await?;
    context
        .check_source_size(bytes.len())
        .map_err(SourceFailure::Diagnostic)?;
    budget
        .charge_imports(&[(ImportResource::SourceBytes, bytes.len())])
        .map_err(SourceFailure::Diagnostic)?;
    String::from_utf8(bytes).map_err(|error| {
        SourceFailure::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

/// The run's package project and the importing directory it sees: found from
/// `base`, the importer's directory, at the first package or URL import; a
/// module imported by URL imports as the project root does.
async fn project(
    base: Option<&Path>,
    span: &Span,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<(Arc<crate::core::packages::Project>, PathBuf)> {
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
    let importer = match base {
        Some(base) => Some(context.canonicalize_source(base).await.map_err(
            |error| match error {
                SourceFailure::Io(error) => {
                    failure(context, format_args!("{}: {error}", base.display()))
                }
                SourceFailure::Diagnostic(error) => related(context, error),
            },
        )?),
        None => None,
    };
    let project = match (context.modules.project.clone(), &importer) {
        (Some(project), _) => project,
        (None, Some(importer)) => {
            let project = Arc::new(context.load_project(importer).await.map_err(
                |error| match error {
                    SourceFailure::Io(error) => failure(context, format_args!("{error}")),
                    SourceFailure::Diagnostic(error) => related(context, error),
                },
            )?);
            context.modules.project = Some(Arc::clone(&project));
            project
        }
        (None, None) => {
            return Err(failure(
                context,
                format_args!("a module imported by URL needs the project that named it"),
            ))
        }
    };
    let importer = importer.unwrap_or_else(|| project.root().to_owned());
    Ok((project, importer))
}

/// The canonical path of the local file `path` names, relative to the source
/// that imports it, recorded once per run so that a repeated import reads no
/// metadata.
async fn resolve(
    path: &str,
    span: &Span,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<PathBuf> {
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
    let importer = span.source().name();
    // A module imported by URL is named by its URL; it has no directory.
    let from_url = crate::core::packages::is_url(importer);
    let base = if from_url {
        None
    } else {
        let importer = Path::new(importer);
        Some(if importer.is_absolute() {
            importer.parent().unwrap_or(Path::new("/")).to_owned()
        } else {
            context
                .working_directory
                .as_ref()
                .map_err(|reason| failure(context, format_args!("{reason}")))?
                .join(importer.parent().unwrap_or(Path::new("")))
        })
    };
    let (requested, package) = if crate::core::packages::is_url(path) {
        // A URL names a file the project's manifest pins, fetched into the cache.
        let (project, _) = project(base.as_deref(), span, import_site, context).await?;
        let file = project
            .url_file(path)
            .map_err(|reason| failure(context, format_args!("{reason}")))?;
        (file, None)
    } else if let Some(parsed) = crate::core::packages::package_path(path) {
        // `@name/file` names a file in a package the project's lockfile resolved.
        let (name, file) = parsed.map_err(|reason| failure(context, format_args!("{reason}")))?;
        let (project, importer) = project(base.as_deref(), span, import_site, context).await?;
        let requested = project
            .resolve(&importer, name, &file)
            .map_err(|reason| failure(context, format_args!("{reason}")))?;
        let directory = project.directory(name).map(Path::to_owned);
        (requested, directory)
    } else if let Some(base) = base {
        (base.join(path), None)
    } else {
        return Err(failure(
            context,
            format_args!(
                "`{path}` is a relative path, but a module imported by URL imports only URLs and packages"
            ),
        ));
    };
    if let Some(canonical) = context.modules.resolved.get(&requested) {
        return Ok(canonical.clone());
    }
    let canonical = context
        .canonicalize_source(&requested)
        .await
        .map_err(|error| match error {
            SourceFailure::Io(error) => {
                failure(context, format_args!("{}: {error}", requested.display()))
            }
            SourceFailure::Diagnostic(error) => related(context, error),
        })?;
    // A link inside a path package must not lead out of it.
    if let Some(directory) = package {
        if !canonical.starts_with(&directory) {
            return Err(failure(
                context,
                format_args!(
                    "`{path}` leads outside its package, to {}",
                    canonical.display()
                ),
            ));
        }
    }
    if let Some(budget) = &context.budget {
        let bytes = requested
            .as_os_str()
            .len()
            .checked_add(canonical.as_os_str().len())
            .ok_or_else(|| related(context, budget.import_limit(ImportResource::MetadataBytes)))?;
        budget
            .charge_imports(&[
                (ImportResource::Paths, 1),
                (ImportResource::MetadataBytes, bytes),
            ])
            .map_err(|error| related(context, error))?;
    }
    context
        .modules
        .resolved
        .insert(requested, canonical.clone());
    Ok(canonical)
}

async fn load_module(
    path: &str,
    span: &Span,
    import_site: &Span,
    context: &mut Context,
) -> EvaluationResult<Arc<LoadedModule>> {
    let failure = |context: &Context, reason: std::fmt::Arguments<'_>| {
        context.import_error(BWErr::ImportRead, reason, span, import_site)
    };
    // Constructed import errors already include this site in admission. Other
    // failures acquire the site exactly once as they cross this import boundary.
    let related = |context: &Context, error: Diagnostic| {
        RuntimeDiagnostic::from(error).with_related_in(
            "imported here",
            import_site,
            context.budget.as_ref(),
            Some((span, false)),
            context.calls.iter().map(|record| &record.frame),
        )
    };
    if let Some(Err(reason)) = crate::core::packages::package_path(path) {
        return Err(failure(context, format_args!("{reason}")));
    }
    if Path::new(path).extension().and_then(|value| value.to_str()) != Some("botwork") {
        return Err(failure(
            context,
            format_args!("`{path}` must name a local .botwork file"),
        ));
    }
    let canonical = resolve(path, span, import_site, context).await?;
    if let Some(start) = context
        .loading
        .iter()
        .position(|loading| *loading == canonical)
    {
        let chain = ImportChain {
            loading: &context.loading[start..],
            repeated: &canonical,
        };
        return Err(context.import_error(
            BWErr::ImportCycle,
            format_args!("{chain}"),
            span,
            import_site,
        ));
    }
    if let Some(module) = context.modules.loaded.get(&canonical) {
        return Ok(Arc::clone(module));
    }
    context
        .check_import_depth()
        .map_err(|error| related(context, error))?;
    check_dependency_depth(context, 0).map_err(|error| related(context, error))?;
    let source = read_module_source(&canonical, context)
        .await
        .map_err(|error| match error {
            SourceFailure::Io(error) => {
                failure(context, format_args!("{}: {error}", canonical.display()))
            }
            SourceFailure::Diagnostic(error) => related(context, error),
        })?;
    // A module imported by URL is named by its URL, not its place in the cache.
    let url = context
        .modules
        .project
        .as_ref()
        .and_then(|project| project.url_of(&canonical))
        .map(str::to_owned);
    let source_name = match &url {
        Some(url) => url.as_str(),
        None => canonical
            .to_str()
            .ok_or_else(|| failure(context, format_args!("Module paths must be valid UTF-8")))?,
    };
    let program = context
        .parse_module(source_name, &source)
        .map_err(|error| {
            error.with_related("imported here", import_site, context.budget.as_ref())
        })?;
    let mut module_context =
        prepare_module_context(context, &canonical).map_err(|error| related(context, error))?;
    let result = execution::evaluate_program_runtime(&program, &mut module_context).await;
    merge_cache(context, &mut module_context);
    result.map_err(|error| {
        error.with_related("imported here", import_site, context.budget.as_ref())
    })?;
    let module = Arc::new(LoadedModule {
        frame: module_context.frames.remove(0),
    });
    context
        .modules
        .loaded
        .insert(canonical, Arc::clone(&module));
    Ok(module)
}

// Keep snapshot measurement/selection storage off recursive module-loading frames.
#[inline(never)]
fn prepare_module_context(context: &Context, canonical: &Path) -> DiagnosticResult<Context> {
    let natives: Vec<_> = context
        .statement_signatures()
        .into_iter()
        .filter_map(|metadata| {
            context
                .get_statement_ref(metadata.normalized())
                .map(|(statement, _)| statement)
                .filter(|statement| {
                    matches!(
                        statement,
                        StmtType::Native { .. } | StmtType::Operation { .. }
                    )
                })
        })
        .collect();
    let mut size = context.isolated_snapshot_size();
    size.entries(natives.len());
    size.path(canonical);
    context.charge_snapshot(size)?;
    let mut module_context = isolated(Frame::default(), context);
    // Modules inherit visible host operations, never caller variables/custom definitions.
    for statement in natives {
        let key = statement.registry().map_or_else(
            || Arc::from(statement.metadata().normalized()),
            |reservation| Arc::clone(&reservation.key),
        );
        module_context.frames[0]
            .statements
            .insert(key, statement.clone());
    }
    module_context.loading.push(canonical.to_owned());
    Ok(module_context)
}

fn isolated(frame: Frame, context: &Context) -> Context {
    Context {
        frames: vec![frame],
        current: 0,
        calls: context.calls.clone(),
        handlers: vec![],
        modules: context.modules.clone(),
        loading: context.loading.clone(),
        working_directory: context.working_directory.clone(),
        environment: context.environment.clone(),
        budget: context.budget.as_ref().map(RunBudget::shared),
        asynchronous: context.asynchronous,
        worker_control: None,
        trace_statements: false,
        #[cfg(test)]
        expression_visits: Default::default(),
    }
}

fn merge_cache(context: &mut Context, module_context: &mut Context) {
    // The caller is suspended during isolated execution; the child contains its
    // complete cache plus new entries. Transfer ownership without another grow/copy.
    context.modules = std::mem::take(&mut module_context.modules);
}

pub(super) async fn invoke_imported(
    call: &Call,
    module: &LoadedModule,
    exported: &str,
    arguments: Vec<TemporaryValue>,
    import_site: &Span,
    context: &mut Context,
) -> TemporaryResult {
    let mut size = context.isolated_snapshot_size();
    module.frame.snapshot_size(&mut size);
    context.charge_snapshot(size).map_err(|error| {
        RuntimeDiagnostic::from(error).with_related_in(
            "imported here",
            import_site,
            context.budget.as_ref(),
            Some((&call.span, false)),
            context.calls.iter().map(|record| &record.frame),
        )
    })?;
    let mut module_context = isolated(module.frame.clone(), context);
    let (definition, owner) = module_context.get_statement(exported).ok_or_else(|| {
        module_context.import_error(
            BWErr::StatementNotDefined,
            format_args!("{exported}"),
            &call.span,
            import_site,
        )
    })?;
    let result =
        execution::invoke_resolved(call, definition, owner, arguments, &mut module_context)
            .await
            .map_err(|error| {
                error.with_related("imported here", import_site, context.budget.as_ref())
            });
    merge_cache(context, &mut module_context);
    result
}
