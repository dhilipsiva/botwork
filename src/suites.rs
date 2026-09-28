//! CLI suite discovery is side-effect free; only admitted cases create contexts.
use super::{batch, Args, BWErr, CliError, Context, Diagnostic};
use botwork::core::suite::{self, SelectedCase, Selection, Suite};
use std::{
    io::{self, Read},
    path::PathBuf,
    sync::Arc,
};

mod datasets;
mod history;

pub(super) struct Request {
    pub(super) paths: Vec<PathBuf>,
    pub(super) selection: Selection,
    pub(super) rerun: Option<PathBuf>,
    pub(super) failures: Option<PathBuf>,
    pub(super) list: bool,
}

impl From<&Args> for Request {
    fn from(args: &Args) -> Self {
        Self {
            paths: args.suite.clone(),
            selection: Selection {
                cases: args.case.clone(),
                tags: args.tag.clone(),
                exclude_tags: args.exclude_tag.clone(),
                failed: None,
            },
            rerun: args.rerun_failed.clone(),
            failures: args.failures.clone(),
            list: args.list_cases,
        }
    }
}

struct Discovered {
    cases: Vec<SelectedCase>,
    history: Option<history::History>,
}

fn discover(mut request: Request) -> Result<Discovered, CliError> {
    if let Some(path) = &request.rerun {
        request.selection.failed = Some(history::load(path)?);
    }
    let history = request.failures.map(history::History::begin).transpose()?;
    if request.paths.len() > suite::MAX_SUITES {
        return Err(suite::resource("suites", suite::MAX_SUITES).into());
    }
    let mut suites = Vec::new();
    let mut discovery = datasets::Discovery::default();
    let mut case_count = 0;
    for path in request.paths {
        let source = discovery.read(&path, false)?;
        let parsed = Suite::parse(&path.display().to_string(), &source)?;
        for definition in parsed.datasets() {
            if let Some(data) = definition.data() {
                discovery.admit(data)?;
            }
        }
        let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let parsed = parsed
            .resolve_datasets(|reference, span| discovery.load(&directory.join(reference), span))?;
        case_count += parsed.run_count()?;
        if case_count > suite::MAX_SELECTED_CASES {
            return Err(suite::resource("discovered cases", suite::MAX_SELECTED_CASES).into());
        }
        suites.push(Arc::new(parsed));
    }
    let cases = request.selection.select(&suites)?;
    Ok(Discovered { cases, history })
}

pub(super) async fn run(
    request: Request,
    jobs: usize,
    configuration: batch::Configuration,
) -> Result<(), CliError> {
    let listing = request.list;
    let discovered = tokio::task::spawn_blocking(move || discover(request))
        .await
        .map_err(|_| {
            Diagnostic::new(BWErr::AsyncRuntime("Suite discovery worker failed".into()))
        })??;
    if listing {
        return tokio::task::spawn_blocking(move || {
            let context = Context::with_limits(configuration.limits)?;
            let mut stdout = io::stdout().lock();
            for case in discovered.cases {
                let mut record = serde_json::json!({"id": case.id(), "suite": case.suite().metadata().name(), "name": case.display_name(), "tags": case.tags()});
                if let Some(row) = case.row() {
                    record["case"] = case.case_id().into();
                    record["dataset"] = case.dataset().expect("row dataset").id().into();
                    record["row"] = serde_json::json!({"id": row.metadata().id(), "name": row.metadata().name()});
                }
                context.write_output(&mut stdout, format_args!("{record}\n"))?;
            }
            Ok(())
        }).await.map_err(|_| Diagnostic::new(BWErr::AsyncRuntime("Case listing worker failed".into())))?;
    }
    let outcome = batch::run_cases(discovered.cases, jobs, configuration).await?;
    if let Some(history) = discovered.history {
        let failed = outcome.failed_cases.clone();
        tokio::task::spawn_blocking(move || history.finish(failed))
            .await
            .map_err(|_| {
                Diagnostic::new(BWErr::AsyncRuntime("Failed-case writer failed".into()))
            })??;
    }
    outcome.result()
}
