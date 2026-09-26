use super::*;
use std::{fs, path::Path};

#[derive(Clone, Default)]
pub(super) struct ModuleCache {
    loaded: HashMap<PathBuf, Arc<LoadedModule>>,
    resolved: HashMap<PathBuf, PathBuf>,
}

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
    if let Some(original) = frame.namespaces.get(&normalized) {
        return Err(namespace_collision(&normalized, original, &namespace.span));
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
    let frame = &mut context.frames[context.current];
    // Publish the complete namespace only after initialization succeeds.
    for (exported, statement) in &module.frame.statements {
        if matches!(statement, StmtType::Native { .. }) {
            continue;
        }
        let metadata = Arc::new(statement.metadata().qualified(&namespace.text));
        frame.statements.insert(
            metadata.normalized().into(),
            StmtType::Imported {
                module: Arc::clone(&module),
                exported: exported.clone(),
                metadata,
                import_site: import_site.clone(),
            },
        );
    }
    frame.namespaces.insert(normalized, import_site.clone());
    Ok(Literal::None)
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
    context
        .modules
        .resolved
        .insert(requested, canonical.clone());
    if let Some(module) = context.modules.loaded.get(&canonical) {
        return Ok(Arc::clone(module));
    }
    let source = context
        .read_source(&canonical)
        .map_err(|error| match error {
            SourceFailure::Io(error) => failure(format!("{}: {error}", canonical.display())),
            SourceFailure::Diagnostic(error) => error.at(span),
        })?;
    let source_name = canonical
        .to_str()
        .ok_or_else(|| failure("Module paths must be valid UTF-8".into()))?;
    let program = context.parse_source(source_name, &source)?;
    let mut module_context = isolated(Frame::default(), context);
    // Modules inherit visible host operations, never caller variables/custom definitions.
    for metadata in context.statement_signatures() {
        let (statement, _) = context
            .get_statement(metadata.normalized())
            .expect("visible entry");
        if matches!(statement, StmtType::Native { .. }) {
            module_context.frames[0]
                .statements
                .insert(metadata.normalized().into(), statement);
        }
    }
    module_context.loading.push(canonical.clone());
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
    context
        .modules
        .loaded
        .extend(module_context.modules.loaded.drain());
    context
        .modules
        .resolved
        .extend(module_context.modules.resolved.drain());
}

pub(super) fn invoke_imported(
    call: &Call,
    module: &LoadedModule,
    exported: &str,
    arguments: Vec<Literal>,
    import_site: &Span,
    context: &mut Context,
) -> RuntimeResult {
    let mut module_context = isolated(module.frame.clone(), context);
    let (definition, owner) = module_context
        .get_statement(exported)
        .ok_or_else(|| BWErr::StatementNotDefined(exported.into()))?;
    let result = invoke_resolved(call, definition, owner, arguments, &mut module_context)
        .map_err(|error| error.with_related("imported here", import_site));
    merge_cache(context, &mut module_context);
    result
}
