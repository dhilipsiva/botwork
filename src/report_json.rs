//! The versioned JSON report and the HTML report: one document per invocation,
//! built from run records and the verdict that decides the exit status.
use super::{atomic_file::AtomicFile, BWErr, CliError, Diagnostic};

mod html;
mod journal;
use botwork::core::{
    acceptance::{CaseStatus, CaseTotals, Delivery, RunVerdict, SkipReason},
    report::{
        timestamp, ArtifactRecord, ErrorRecord, Event, EventObserver, EventRecord, RecordLimits,
        RecordOptions, Recording, RunIdentity, RunRecord,
    },
};
use journal::{Journal, Line, Stored};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Instant, SystemTime},
};

pub(super) const FORMAT: &str = "botwork-report";
pub(super) const VERSION: u32 = 1;
/// Serialized run details beyond this budget are reduced to their summaries.
pub(super) const MAX_RUN_BYTES: usize = 32 * 1024 * 1024;
/// An existing file is only replaced after it is recognized as a report.
const MAX_EXISTING_BYTES: usize = 64 * 1024 * 1024;

/// Per-run retention for reports, so large suites stay reviewable.
pub(super) fn limits() -> RecordLimits {
    RecordLimits {
        statements: 256,
        logs: 64,
        log_bytes: 16 * 1024,
        log_record_bytes: 1024,
        message_bytes: 2048,
        causes: 16,
        artifacts: 64,
    }
}

/// Start recording one CLI run with report retention bounds.
pub(super) fn recording(identity: RunIdentity, observer: Option<EventObserver>) -> Recording {
    Recording::start(RecordOptions {
        identity,
        limits: limits(),
        observer,
        ..RecordOptions::default()
    })
}

/// The only event of selected work that never started.
pub(super) fn skipped_event(identity: RunIdentity, reason: SkipReason) -> EventRecord {
    EventRecord {
        sequence: 0,
        event: Event::RunSkipped {
            identity,
            reason,
            recorded_at: timestamp(SystemTime::now()),
        },
    }
}

/// The diagnostic a record keeps for a CLI failure. Entry-file read failures,
/// which have no language code, use BW7003 as embedded runs do.
pub(super) fn diagnostic(error: &CliError) -> Diagnostic {
    match error {
        CliError::Script(error) => error.clone(),
        CliError::SourceLimit { source, .. } => (**source).clone(),
        error => Diagnostic::new(BWErr::SourceRead(error.to_string())),
    }
}

/// What reconciliation found in a journal.
pub(super) struct Reconciled {
    pub(super) recorded: usize,
    pub(super) interrupted: usize,
    pub(super) selected: Option<usize>,
    /// Bytes of a final line that the termination cut short.
    pub(super) torn: usize,
}

/// A run record with its CLI occurrence number.
#[derive(Serialize)]
struct Entry<'a> {
    number: usize,
    #[serde(flatten)]
    record: &'a RunRecord,
}

/// The identity and outcome of a run whose details exceeded the report budget.
#[derive(Serialize, Deserialize)]
struct Summary {
    number: usize,
    identity: RunIdentity,
    status: CaseStatus,
    complete: bool,
    error: Option<String>,
    details_omitted: bool,
}

/// What the report retains for one run.
enum Kept {
    Full(Box<RunRecord>),
    Summary(Summary),
}

#[derive(Serialize)]
#[serde(untagged)]
enum Run<'a> {
    Full(Entry<'a>),
    Summary(&'a Summary),
}

/// Counts serialized bytes without retaining them.
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Fixture {
    suite: String,
    status: CaseStatus,
    error: Option<ErrorRecord>,
}

#[derive(Serialize)]
struct Document<'a> {
    format: &'static str,
    version: u32,
    /// True once the document is final and every selected run has a record.
    complete: bool,
    mode: &'a str,
    started_at: &'a str,
    finished_at: Option<String>,
    duration_us: Option<u64>,
    exit_code: Option<u8>,
    verdict: Option<&'a RunVerdict>,
    runs: Vec<Run<'a>>,
    omitted_run_details: usize,
    fixtures: &'a [Fixture],
}

#[derive(Default)]
struct Collected {
    runs: BTreeMap<usize, Kept>,
    fixtures: Vec<Fixture>,
    /// Serialized bytes of the full run details retained so far.
    retained: usize,
}

pub(super) struct Report {
    json: Option<AtomicFile>,
    html: Option<AtomicFile>,
    mode: String,
    started_at: String,
    /// Absent for a reconciled report, whose duration is unknown.
    start: Option<Instant>,
    /// Absent for a reconciled report, whose journal is being consumed.
    journal: Option<Journal>,
    /// Serialized run bytes retained in full; `MAX_RUN_BYTES` outside tests.
    budget: usize,
    collected: Mutex<Collected>,
}

fn recognized(path: &Path) -> Result<(), CliError> {
    let mut bytes = Vec::new();
    let read = OpenOptions::new().read(true).open(path).and_then(|file| {
        file.take(MAX_EXISTING_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
    });
    let rejected = |reason: &str| -> CliError {
        Diagnostic::new(BWErr::OutputError(format!(
            "JSON report {}: {reason}",
            path.display()
        )))
        .into()
    };
    read.map_err(|error| rejected(&error.to_string()))?;
    let value: serde_json::Value = (bytes.len() <= MAX_EXISTING_BYTES)
        .then(|| serde_json::from_slice(&bytes).ok())
        .flatten()
        .ok_or_else(|| rejected("an existing file that is not a JSON report is never replaced"))?;
    if value["format"] != FORMAT {
        return Err(rejected(
            "an existing file that is not a JSON report is never replaced",
        ));
    }
    Ok(())
}

impl Report {
    /// Lock the outputs and publish incomplete markers before any run starts.
    pub(super) fn begin(
        json: Option<PathBuf>,
        html: Option<PathBuf>,
        mode: &'static str,
    ) -> Result<Self, CliError> {
        let json = json
            .map(|path| AtomicFile::begin(path, "JSON report", "botwork-report", recognized))
            .transpose()?;
        let html = html
            .map(|path| {
                AtomicFile::begin(path, "HTML report", "botwork-report-html", html::recognized)
            })
            .transpose()?;
        let started_at = timestamp(SystemTime::now());
        let first = json.as_ref().or(html.as_ref()).expect("an output");
        let journal = Journal::create(
            journal::path_for(first.path()),
            &Line::Header {
                format: journal::FORMAT,
                version: journal::VERSION,
                mode,
                started_at: &started_at,
                json: json.as_ref().map(AtomicFile::path),
                html: html.as_ref().map(AtomicFile::path),
            },
        )?;
        let report = Self {
            json,
            html,
            mode: mode.to_owned(),
            started_at,
            start: Some(Instant::now()),
            journal: Some(journal),
            budget: MAX_RUN_BYTES,
            collected: Mutex::default(),
        };
        report.publish(None, false)?;
        Ok(report)
    }

    fn journal(&self, line: &Line<'_>) {
        if let Some(journal) = &self.journal {
            journal.append(line);
        }
    }

    /// Whether journaling failed, so a forced termination could not have been
    /// reconciled; delivery is then incomplete.
    pub(super) fn journal_failed(&self) -> bool {
        self.journal
            .as_ref()
            .is_some_and(|journal| journal.failure().is_some())
    }

    /// The number of runs selected, once discovery knows it.
    pub(super) fn selected(&self, count: usize) {
        self.journal(&Line::Selected { count });
    }

    /// Run `number` started; reconciliation treats it as interrupted until it
    /// has a record.
    pub(super) fn started(&self, number: usize, identity: &RunIdentity) {
        self.journal(&Line::Started {
            number,
            identity,
            started_at: &timestamp(SystemTime::now()),
        });
    }

    fn collected(&self) -> std::sync::MutexGuard<'_, Collected> {
        self.collected
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Keep a finished run, attaching the artifacts exported after it finished.
    pub(super) fn run(&self, number: usize, mut record: RunRecord, artifacts: &[PathBuf]) {
        for path in artifacts {
            let artifact = ArtifactRecord {
                kind: "assertion".into(),
                path: path.display().to_string(),
            };
            if record.artifacts.len() < limits().artifacts {
                record.artifacts.push(artifact);
            } else {
                record.omitted_artifacts += 1;
            }
        }
        self.keep(number, record);
    }

    /// Retain full details while they fit the budget, in the order runs finish,
    /// so memory stays bounded however many runs are selected.
    fn keep(&self, number: usize, record: RunRecord) {
        let mut counter = Counter(0);
        let size = serde_json::to_writer(
            &mut counter,
            &Entry {
                number,
                record: &record,
            },
        )
        .map_or(usize::MAX, |()| counter.0);
        let mut collected = self.collected();
        let kept = if size <= self.budget.saturating_sub(collected.retained) {
            collected.retained += size;
            self.journal(&Line::Run {
                number,
                record: &record,
            });
            Kept::Full(Box::new(record))
        } else {
            let summary = Summary {
                number,
                status: record.status,
                complete: record.complete,
                error: record.error.as_ref().map(|error| error.code.to_owned()),
                identity: record.identity,
                details_omitted: true,
            };
            self.journal(&Line::Summary { summary: &summary });
            Kept::Summary(summary)
        };
        collected.runs.insert(number, kept);
    }

    /// Selected work that never started has one skipped record.
    pub(super) fn skipped(&self, number: usize, event: &EventRecord) {
        let record = RunRecord::from_events(RunIdentity::default(), limits(), [event])
            .expect("a skipped record has one event");
        self.keep(number, record);
    }

    pub(super) fn fixture(&self, suite: &str, status: CaseStatus, error: Option<ErrorRecord>) {
        let fixture = Fixture {
            suite: suite.to_owned(),
            status,
            error,
        };
        self.journal(&Line::Fixture { fixture: &fixture });
        self.collected().fixtures.push(fixture);
    }

    /// Fold the journal an interrupted invocation left beside `path` into its
    /// final report(s). Started runs without a record become interrupted, and
    /// the verdict is interrupted, so partial output never reads as a pass.
    pub(super) fn reconcile(path: &Path) -> Result<Reconciled, CliError> {
        let journal_path = journal::path_for(path);
        let contents = journal::read(&journal_path)?;
        let mut entries = contents.entries.into_iter();
        let Some(Stored::Header {
            mode,
            started_at,
            json,
            html,
            ..
        }) = entries.next()
        else {
            unreachable!("the journal reader checks the header")
        };
        if !matches!(mode.as_str(), "file" | "batch" | "suites") {
            return Err(Diagnostic::new(BWErr::OutputError(format!(
                "Report journal {}: unknown mode {mode:?}",
                journal_path.display()
            )))
            .into());
        }
        // The locks also prove that the invocation that wrote the journal ended.
        let json = json
            .map(|path| AtomicFile::begin(path, "JSON report", "botwork-report", recognized))
            .transpose()?;
        let html = html
            .map(|path| {
                AtomicFile::begin(path, "HTML report", "botwork-report-html", html::recognized)
            })
            .transpose()?;
        if json.is_none() && html.is_none() {
            return Err(Diagnostic::new(BWErr::OutputError(format!(
                "Report journal {}: the header names no report",
                journal_path.display()
            )))
            .into());
        }
        let report = Self {
            json,
            html,
            mode,
            started_at,
            start: None,
            journal: None,
            budget: MAX_RUN_BYTES,
            collected: Mutex::default(),
        };
        let mut selected = None;
        let mut started = BTreeMap::new();
        {
            let mut collected = report.collected();
            for entry in entries {
                match entry {
                    Stored::Header { .. } => unreachable!("one header"),
                    Stored::Selected { count } => selected = Some(count),
                    Stored::Started {
                        number,
                        identity,
                        started_at,
                    } => {
                        started.insert(number, (identity, started_at));
                    }
                    Stored::Run { number, record } => {
                        collected.runs.insert(number, Kept::Full(record));
                    }
                    Stored::Summary { summary } => {
                        collected
                            .runs
                            .insert(summary.number, Kept::Summary(summary));
                    }
                    Stored::Fixture { fixture } => collected.fixtures.push(fixture),
                }
            }
            let mut interrupted = 0;
            for (number, (identity, started_at)) in started {
                if collected.runs.contains_key(&number) {
                    continue;
                }
                let event = EventRecord {
                    sequence: 0,
                    event: Event::RunStarted {
                        identity: identity.clone(),
                        started_at,
                    },
                };
                let record = RunRecord::from_events(identity, limits(), [&event])
                    .expect("a started record has one event");
                collected.runs.insert(number, Kept::Full(Box::new(record)));
                interrupted += 1;
            }
            let mut totals = CaseTotals::default();
            for kept in collected.runs.values() {
                totals.record(match kept {
                    Kept::Full(record) => record.status,
                    Kept::Summary(summary) => summary.status,
                })?;
            }
            let fixture_failures = collected
                .fixtures
                .iter()
                .filter(|fixture| fixture.status.failed())
                .count();
            let recorded = collected.runs.len();
            drop(collected);
            let verdict = totals.finish(fixture_failures, Delivery::Interrupted);
            let complete = selected == Some(recorded);
            report.publish(Some(&verdict), complete)?;
            std::fs::remove_file(&journal_path).map_err(|error| {
                CliError::from(Diagnostic::new(BWErr::OutputError(format!(
                    "Report journal {}: {error}",
                    journal_path.display()
                ))))
            })?;
            Ok(Reconciled {
                recorded,
                interrupted,
                selected,
                torn: contents.torn,
            })
        }
    }

    fn first(&self) -> &AtomicFile {
        self.json
            .as_ref()
            .or(self.html.as_ref())
            .expect("an output")
    }

    /// Publish the final report. With complete delivery every selected run must
    /// have a record; after an interruption or a delivery failure, runs that
    /// never started have none and the report says so.
    pub(super) fn finish(&self, verdict: &RunVerdict, selected: usize) -> Result<(), CliError> {
        let recorded = self.collected().runs.len();
        if recorded != selected && verdict.delivery() == Delivery::Complete {
            return Err(self.first().error(format_args!(
                "{recorded} of {selected} runs have records; the report stays incomplete"
            )));
        }
        self.publish(Some(verdict), recorded == selected)?;
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        let failed = journal.failure();
        journal.finish()?;
        match failed {
            Some(error) => Err(self.first().error(format_args!(
                "the journal failed ({error}), so a forced termination could not have been reconciled"
            ))),
            None => Ok(()),
        }
    }

    fn publish(&self, verdict: Option<&RunVerdict>, complete: bool) -> Result<(), CliError> {
        let collected = self.collected();
        let mut omitted = 0;
        let runs = collected
            .runs
            .iter()
            .map(|(&number, kept)| match kept {
                Kept::Full(record) => Run::Full(Entry { number, record }),
                Kept::Summary(summary) => {
                    omitted += 1;
                    Run::Summary(summary)
                }
            })
            .collect();
        let finished = verdict.map(|_| SystemTime::now());
        let document = Document {
            format: FORMAT,
            version: VERSION,
            complete: verdict.is_some() && complete,
            mode: &self.mode,
            started_at: &self.started_at,
            finished_at: finished.map(timestamp),
            duration_us: finished
                .and(self.start)
                .map(|start| u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)),
            exit_code: verdict.map(RunVerdict::exit_code),
            verdict,
            runs,
            omitted_run_details: omitted,
            fixtures: &collected.fixtures,
        };
        // Attempt every output; the first failure is the result.
        let json = self
            .json
            .as_ref()
            .map_or(Ok(()), |json| json.write(&document));
        let html = self.html.as_ref().map_or(Ok(()), |html| {
            html.write_bytes(html::render(&document, html.directory()).as_bytes())
        });
        json.and(html)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use botwork::core::run::{Engine, RunOptions};
    use std::fs;

    fn record(name: &str, source: &str) -> RunRecord {
        let options = RunOptions {
            record: Some(RecordOptions {
                identity: RunIdentity::new(name, name),
                limits: limits(),
                ..RecordOptions::default()
            }),
            ..RunOptions::default()
        };
        Engine::default()
            .run_source(name, source, options)
            .record
            .expect("requested record")
    }

    fn verdict(statuses: &[CaseStatus]) -> RunVerdict {
        let mut totals = CaseTotals::default();
        for status in statuses {
            totals.record(*status).unwrap();
        }
        totals.finish(0, Delivery::Complete)
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn details_beyond_the_run_budget_become_summaries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let mut report = Report::begin(Some(path.clone()), None, "batch").unwrap();
        // Identical sizes: exactly one run fits, and only when the budget is
        // charged. Run 3 finishes first, so it keeps its details.
        let mut failed = record("run", "Log |\"one\"|\nAssert |false|");
        failed.identity = RunIdentity::new("run-1", "run");
        report.budget = serde_json::to_vec(&Entry {
            number: 1,
            record: &failed,
        })
        .unwrap()
        .len();
        for number in [3, 1, 2] {
            let mut record = failed.clone();
            record.identity = RunIdentity::new(format!("run-{number}"), "run");
            report.run(number, record, &[]);
        }
        report
            .finish(&verdict(&[CaseStatus::Failed; 3]), 3)
            .unwrap();
        let value = read(&path);
        assert_eq!(value["complete"], true);
        assert_eq!(value["omitted_run_details"], 2);
        let runs = value["runs"].as_array().unwrap();
        assert_eq!(runs[2]["number"], 3);
        assert_eq!(runs[2]["format"], "botwork-run");
        assert_eq!(runs[2]["logs"][0]["text"], "one");
        for (index, summary) in runs[..2].iter().enumerate() {
            assert_eq!(summary["number"], index + 1);
            assert_eq!(summary["identity"]["id"], format!("run-{}", index + 1));
            assert_eq!(summary["status"], "failed");
            assert_eq!(summary["complete"], true);
            assert_eq!(summary["error"], "BW9001");
            assert_eq!(summary["details_omitted"], true);
            assert!(summary.get("statements").is_none());
        }
    }

    #[test]
    fn missing_records_keep_the_report_incomplete() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let report = Report::begin(Some(path.clone()), None, "batch").unwrap();
        let marker = read(&path);
        assert_eq!(marker["complete"], false);
        assert!(marker["exit_code"].is_null() && marker["verdict"].is_null());
        report.run(1, record("only", "No Operation"), &[]);
        let error = report
            .finish(&verdict(&[CaseStatus::Succeeded, CaseStatus::Succeeded]), 2)
            .unwrap_err();
        assert!(error.to_string().contains("1 of 2 runs"), "{error}");
        assert_eq!(read(&path), marker);
    }

    #[test]
    fn artifacts_beyond_the_limit_are_counted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let report = Report::begin(Some(path.clone()), None, "file").unwrap();
        let artifacts: Vec<_> = (0..limits().artifacts + 3)
            .map(|index| PathBuf::from(format!("assertion-{index}.json")))
            .collect();
        report.run(1, record("many", "Assert |false|"), &artifacts);
        report.finish(&verdict(&[CaseStatus::Failed]), 1).unwrap();
        let run = &read(&path)["runs"][0];
        assert_eq!(
            run["artifacts"].as_array().unwrap().len(),
            limits().artifacts
        );
        assert_eq!(run["artifacts"][0]["kind"], "assertion");
        assert_eq!(run["artifacts"][0]["path"], "assertion-0.json");
        assert_eq!(run["omitted_artifacts"], 3);
    }

    #[test]
    fn only_existing_reports_are_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        for existing in ["not json", "{}", r#"{"format":"botwork-failed-cases"}"#] {
            fs::write(&path, existing).unwrap();
            let error = Report::begin(Some(path.clone()), None, "file")
                .err()
                .expect("rejected");
            assert!(error.to_string().contains("never replaced"), "{error}");
            assert_eq!(fs::read_to_string(&path).unwrap(), existing);
        }
        fs::write(&path, r#"{"format":"botwork-report","version":1}"#).unwrap();
        drop(Report::begin(Some(path.clone()), None, "file").unwrap());
        assert_eq!(read(&path)["complete"], false);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let error = Report::begin(Some(path), None, "file")
            .err()
            .expect("rejected");
        assert!(error.to_string().contains("ordinary file"), "{error}");
    }

    fn interrupted(statuses: &[CaseStatus]) -> RunVerdict {
        let mut totals = CaseTotals::default();
        for status in statuses {
            totals.record(*status).unwrap();
        }
        totals.finish(0, Delivery::Interrupted)
    }

    #[test]
    fn stopped_deliveries_publish_final_reports_without_every_run() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let report = Report::begin(Some(path.clone()), None, "batch").unwrap();
        report.selected(3);
        report.started(1, &RunIdentity::new("one", "one"));
        report.run(1, record("one", "No Operation"), &[]);
        report
            .finish(&interrupted(&[CaseStatus::Succeeded]), 3)
            .unwrap();
        let value = read(&path);
        assert_eq!(value["complete"], false, "two selected runs never started");
        assert_eq!(value["verdict"]["delivery"], "interrupted");
        assert_eq!(value["verdict"]["status"], "interrupted");
        assert_eq!(value["exit_code"], 1);
        assert!(
            !journal::path_for(&path).exists(),
            "a final report needs no journal"
        );
    }

    /// Start a report whose run 1 finished and run 2 was still running when the
    /// invocation ended without finishing its report.
    fn abandoned(directory: &Path) -> (PathBuf, PathBuf) {
        let (json, html) = (directory.join("report.json"), directory.join("report.html"));
        let report = Report::begin(Some(json.clone()), Some(html.clone()), "batch").unwrap();
        report.selected(3);
        report.started(1, &RunIdentity::new("one", "one"));
        report.run(1, record("one", "Log |\"done\"|"), &[]);
        report.started(2, &RunIdentity::new("two", "two"));
        report.fixture("suite", CaseStatus::Succeeded, None);
        drop(report);
        (json, html)
    }

    #[test]
    fn journals_reconcile_started_runs_as_interrupted() {
        let directory = tempfile::tempdir().unwrap();
        let (json, html) = abandoned(directory.path());
        assert_eq!(read(&json)["complete"], false);
        assert!(read(&json)["verdict"].is_null(), "the marker stays");
        let found = Report::reconcile(&json).unwrap();
        assert_eq!(
            (
                found.recorded,
                found.interrupted,
                found.selected,
                found.torn
            ),
            (2, 1, Some(3), 0)
        );
        let value = read(&json);
        assert_eq!(value["complete"], false);
        assert_eq!(value["exit_code"], 1);
        assert!(value["duration_us"].is_null() && value["finished_at"].is_string());
        assert_eq!(value["verdict"]["status"], "interrupted");
        assert_eq!(value["verdict"]["delivery"], "interrupted");
        assert_eq!(value["runs"][0]["status"], "succeeded");
        assert_eq!(value["runs"][0]["logs"][0]["text"], "done");
        let two = &value["runs"][1];
        assert_eq!(
            (two["number"].clone(), two["status"].clone()),
            (2.into(), "interrupted".into())
        );
        assert_eq!(two["complete"], false);
        assert!(two["started_at"].is_string() && two["finished_at"].is_null());
        assert_eq!(value["fixtures"][0]["suite"], "suite");
        let page = fs::read_to_string(&html).unwrap();
        assert!(page.contains("<title>Botwork report: interrupted</title>"));
        assert!(page.contains("This invocation was interrupted."));
        assert!(!journal::path_for(&json).exists());
        assert!(
            Report::reconcile(&json).is_err(),
            "nothing is left to reconcile"
        );
    }

    #[test]
    fn torn_final_lines_are_ignored_and_corrupt_journals_refused() {
        let directory = tempfile::tempdir().unwrap();
        let (json, _) = abandoned(directory.path());
        let journal = journal::path_for(&json);
        let original = fs::read(&journal).unwrap();
        let mut corrupt = original.clone();
        corrupt.extend_from_slice(b"not json\n{\"entry\":\"selected\",\"count\":1}\n");
        fs::write(&journal, &corrupt).unwrap();
        let error = Report::reconcile(&json).err().expect("corrupt journal");
        assert!(error.to_string().contains("line 7"), "{error}");
        assert_eq!(fs::read(&journal).unwrap(), corrupt, "nothing changes");
        assert!(read(&json)["verdict"].is_null());
        let torn = br#"{"entry":"run","num"#;
        fs::write(&journal, [original.as_slice(), torn].concat()).unwrap();
        let found = Report::reconcile(&json).unwrap();
        assert_eq!((found.interrupted, found.torn), (1, torn.len()));
    }

    #[test]
    fn a_pending_journal_blocks_new_reports_but_not_its_reconciliation() {
        let directory = tempfile::tempdir().unwrap();
        let (json, _) = abandoned(directory.path());
        let error = Report::begin(Some(json.clone()), None, "file")
            .err()
            .expect("blocked");
        assert!(error.to_string().contains("--reconcile-report"), "{error}");
        assert!(read(&json)["verdict"].is_null(), "the marker is untouched");
        Report::reconcile(&json).unwrap();
        drop(Report::begin(Some(json), None, "file").unwrap());
    }

    #[test]
    fn reports_without_started_runs_leave_no_journal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let report = Report::begin(Some(path.clone()), None, "suites").unwrap();
        assert!(journal::path_for(&path).exists());
        report.selected(2);
        drop(report);
        assert!(!journal::path_for(&path).exists(), "nothing to reconcile");
        assert!(read(&path)["verdict"].is_null(), "the marker stays");
    }
}
