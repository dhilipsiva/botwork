//! The versioned JSON report: one `botwork-report` document per invocation,
//! built from run records and the verdict that decides the exit status.
use super::{atomic_json::AtomicJson, BWErr, CliError, Diagnostic};
use botwork::core::{
    acceptance::{CaseStatus, RunVerdict, SkipReason},
    report::{
        timestamp, ArtifactRecord, ErrorRecord, Event, EventRecord, RecordLimits, RecordOptions,
        Recording, RunIdentity, RunRecord,
    },
};
use serde::Serialize;
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
pub(super) fn recording(identity: RunIdentity) -> Recording {
    Recording::start(RecordOptions {
        identity,
        limits: limits(),
        ..RecordOptions::default()
    })
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

/// A run record with its CLI occurrence number.
#[derive(Serialize)]
struct Entry<'a> {
    number: usize,
    #[serde(flatten)]
    record: &'a RunRecord,
}

/// The identity and outcome of a run whose details exceeded the report budget.
#[derive(Serialize)]
struct Summary {
    number: usize,
    identity: RunIdentity,
    status: CaseStatus,
    complete: bool,
    error: Option<&'static str>,
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

#[derive(Serialize)]
struct Fixture {
    suite: String,
    status: CaseStatus,
    error: Option<ErrorRecord>,
}

#[derive(Serialize)]
struct Document<'a> {
    format: &'static str,
    version: u32,
    /// True only when every selected run was recorded and delivery completed.
    complete: bool,
    mode: &'static str,
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
    file: AtomicJson,
    mode: &'static str,
    started_at: String,
    start: Instant,
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
    /// Lock the output and publish an incomplete marker before any run starts.
    pub(super) fn begin(path: PathBuf, mode: &'static str) -> Result<Self, CliError> {
        let file = AtomicJson::begin(path, "JSON report", "botwork-report", recognized)?;
        let report = Self {
            file,
            mode,
            started_at: timestamp(SystemTime::now()),
            start: Instant::now(),
            budget: MAX_RUN_BYTES,
            collected: Mutex::default(),
        };
        report.publish(None)?;
        Ok(report)
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
            Kept::Full(Box::new(record))
        } else {
            Kept::Summary(Summary {
                number,
                status: record.status,
                complete: record.complete,
                error: record.error.as_ref().map(|error| error.code),
                identity: record.identity,
                details_omitted: true,
            })
        };
        collected.runs.insert(number, kept);
    }

    /// Selected work that never started has one skipped record.
    pub(super) fn skipped(&self, number: usize, identity: RunIdentity, reason: SkipReason) {
        let event = EventRecord {
            sequence: 0,
            event: Event::RunSkipped {
                identity,
                reason,
                recorded_at: timestamp(SystemTime::now()),
            },
        };
        let record = RunRecord::from_events(RunIdentity::default(), limits(), [&event])
            .expect("a skipped record has one event");
        self.keep(number, record);
    }

    pub(super) fn fixture(&self, suite: &str, result: &Result<(), CliError>) {
        let error = result
            .as_ref()
            .err()
            .map(|error| ErrorRecord::from_diagnostic(&diagnostic(error), &limits()));
        self.collected().fixtures.push(Fixture {
            suite: suite.to_owned(),
            status: error
                .as_ref()
                .map_or(CaseStatus::Succeeded, |error| error.status),
            error,
        });
    }

    /// Publish the complete report. `expected` runs must all have records.
    pub(super) fn finish(&self, verdict: &RunVerdict, expected: usize) -> Result<(), CliError> {
        let recorded = self.collected().runs.len();
        if recorded != expected {
            return Err(self.file.error(format_args!(
                "{recorded} of {expected} runs have records; the report stays incomplete"
            )));
        }
        self.publish(Some(verdict))
    }

    fn publish(&self, verdict: Option<&RunVerdict>) -> Result<(), CliError> {
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
        self.file.write(&Document {
            format: FORMAT,
            version: VERSION,
            complete: verdict.is_some_and(RunVerdict::complete),
            mode: self.mode,
            started_at: &self.started_at,
            finished_at: finished.map(timestamp),
            duration_us: finished
                .map(|_| u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX)),
            exit_code: verdict.map(RunVerdict::exit_code),
            verdict,
            runs,
            omitted_run_details: omitted,
            fixtures: &collected.fixtures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use botwork::core::{
        acceptance::{CaseTotals, Delivery},
        run::{Engine, RunOptions},
    };
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
        let mut report = Report::begin(path.clone(), "batch").unwrap();
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
        let report = Report::begin(path.clone(), "batch").unwrap();
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
        let report = Report::begin(path.clone(), "file").unwrap();
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
            let error = Report::begin(path.clone(), "file").err().expect("rejected");
            assert!(error.to_string().contains("never replaced"), "{error}");
            assert_eq!(fs::read_to_string(&path).unwrap(), existing);
        }
        fs::write(&path, r#"{"format":"botwork-report","version":1}"#).unwrap();
        drop(Report::begin(path.clone(), "file").unwrap());
        assert_eq!(read(&path)["complete"], false);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let error = Report::begin(path, "file").err().expect("rejected");
        assert!(error.to_string().contains("ordinary file"), "{error}");
    }
}
