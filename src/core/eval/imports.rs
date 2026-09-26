use super::*;
use crate::core::run::{ImportResource, SnapshotSize};
use std::{fs, path::Path};

#[derive(Default)]
pub(super) struct ModuleCache {
    loaded: HashMap<PathBuf, Arc<LoadedModule>>,
    resolved: HashMap<PathBuf, PathBuf>,
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
        for (requested, canonical) in &self.resolved {
            size.path(requested);
            size.path(canonical);
        }
    }
}

#[cfg(test)]
mod tests;

pub(super) struct LoadedModule {
    frame: Frame,
}

pub(super) fn namespace_collision(
    namespace: &str,
    original: &Span,
    duplicate: &Span,
) -> Diagnostic {
    Diagnostic::new(BWErr::DuplicateNamespace {
        namespace: namespace.into(),
        original: original.location(),
        duplicate: duplicate.location(),
    })
    .at(duplicate)
    .with_related("first namespace occupant", original)
}

pub(super) fn evaluate_import(
    path: &str,
    path_span: &Span,
    namespace: &Name,
    import_site: &Span,
    context: &mut Context,
) -> RuntimeResult {
    let normalized = ast::normalize_sentence(&namespace.text);
    let frame = &context.frames[context.current];
    if let Some(original) = frame.namespaces.get(normalized.as_str()) {
        return Err(namespace_collision(
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
            &normalized,
            statement.metadata().header(),
            &namespace.span,
        ));
    }
    let module = load_module(path, path_span, context)
        .map_err(|error| error.with_related("imported here", import_site))?;
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
) -> RuntimeResult {
    admit_namespace(module, namespace, normalized, context)
        .map_err(|error| error.at(import_site))?;
    let namespace_registry = context
        .reserve_registry(RegistryPlan::namespace(normalized, import_site))
        .map_err(|error| error.at(import_site))?;
    let namespace_key = namespace_registry.as_ref().map_or_else(
        || Arc::from(normalized),
        |reservation| Arc::clone(&reservation.key),
    );
    let mut exports = Vec::new();
    // Admit every wrapper before copying metadata or publishing the namespace.
    let mut declarations: Vec<_> = module.frame.statements.iter().collect();
    declarations.sort_unstable_by_key(|(left, _)| *left);
    for (exported, statement) in declarations {
        if matches!(statement, StmtType::Native { .. }) {
            continue;
        }
        let registry = context
            .reserve_registry(RegistryPlan::qualified(
                statement.metadata(),
                &namespace.text,
                normalized,
                import_site,
            ))
            .map_err(|error| error.at(import_site))?;
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
        for statement in module
            .frame
            .statements
            .values()
            .filter(|statement| !matches!(statement, StmtType::Native { .. }))
        {
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

fn read_module_source(path: &Path, context: &Context) -> Result<String, SourceFailure> {
    let Some(budget) = &context.budget else {
        return context.read_source(path);
    };
    budget
        .charge_imports(&[
            (ImportResource::Loads, 1),
            (ImportResource::MetadataBytes, path.as_os_str().len()),
        ])
        .map_err(SourceFailure::Diagnostic)?;
    let remaining = budget.import_remaining(ImportResource::SourceBytes);
    let bytes =
        crate::core::run::read_source(path, Some(remaining.min(budget.limits().source_bytes)))?;
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

fn load_module(
    path: &str,
    span: &Span,
    context: &mut Context,
) -> DiagnosticResult<Arc<LoadedModule>> {
    let failure = |reason: String| Diagnostic::new(BWErr::ImportRead(reason)).at(span);
    if path.contains("://")
        || Path::new(path).extension().and_then(|value| value.to_str()) != Some("botwork")
    {
        return Err(failure(format!("`{path}` must name a local .botwork file")));
    }
    let importer = Path::new(span.source().name());
    let base = if importer.is_absolute() {
        importer.parent().unwrap_or(Path::new("/")).to_owned()
    } else {
        context
            .working_directory
            .as_ref()
            .map_err(|reason| failure(reason.clone()))?
            .join(importer.parent().unwrap_or(Path::new("")))
    };
    let requested = base.join(path);
    if let Some(module) = context
        .modules
        .resolved
        .get(&requested)
        .and_then(|canonical| context.modules.loaded.get(canonical))
    {
        return Ok(Arc::clone(module));
    }
    let canonical = fs::canonicalize(&requested)
        .map_err(|error| failure(format!("{}: {error}", requested.display())))?;
    if let Some(start) = context
        .loading
        .iter()
        .position(|loading| *loading == canonical)
    {
        let chain = context.loading[start..]
            .iter()
            .chain(std::iter::once(&canonical))
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" -> ");
        return Err(Diagnostic::new(BWErr::ImportCycle(chain)).at(span));
    }
    if !context.modules.resolved.contains_key(&requested) {
        if let Some(budget) = &context.budget {
            let bytes = requested
                .as_os_str()
                .len()
                .checked_add(canonical.as_os_str().len())
                .ok_or_else(|| budget.import_limit(ImportResource::MetadataBytes).at(span))?;
            budget
                .charge_imports(&[
                    (ImportResource::Paths, 1),
                    (ImportResource::MetadataBytes, bytes),
                ])
                .map_err(|error| error.at(span))?;
        }
        context
            .modules
            .resolved
            .insert(requested, canonical.clone());
    }
    if let Some(module) = context.modules.loaded.get(&canonical) {
        return Ok(Arc::clone(module));
    }
    context
        .check_import_depth()
        .map_err(|error| error.at(span))?;
    check_dependency_depth(context, 0).map_err(|error| error.at(span))?;
    let source = read_module_source(&canonical, context).map_err(|error| match error {
        SourceFailure::Io(error) => failure(format!("{}: {error}", canonical.display())),
        SourceFailure::Diagnostic(error) => error.at(span),
    })?;
    let source_name = canonical
        .to_str()
        .ok_or_else(|| failure("Module paths must be valid UTF-8".into()))?;
    let program = context.parse_source(source_name, &source)?;
    let mut module_context =
        prepare_module_context(context, &canonical).map_err(|error| error.at(span))?;
    let result = evaluate_program_detailed(&program, &mut module_context);
    merge_cache(context, &mut module_context);
    result?;
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
                .filter(|statement| matches!(statement, StmtType::Native { .. }))
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
        #[cfg(test)]
        expression_visits: Default::default(),
    }
}

fn merge_cache(context: &mut Context, module_context: &mut Context) {
    // The caller is suspended during isolated execution; the child contains its
    // complete cache plus new entries. Transfer ownership without another grow/copy.
    context.modules = std::mem::take(&mut module_context.modules);
}

pub(super) fn invoke_imported(
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
        error
            .at(&call.span)
            .with_related("imported here", import_site)
    })?;
    let mut module_context = isolated(module.frame.clone(), context);
    let (definition, owner) = module_context
        .get_statement(exported)
        .ok_or_else(|| BWErr::StatementNotDefined(exported.into()))?;
    let result = invoke_resolved(call, definition, owner, arguments, &mut module_context)
        .map_err(|error| error.with_related("imported here", import_site));
    merge_cache(context, &mut module_context);
    result
}
