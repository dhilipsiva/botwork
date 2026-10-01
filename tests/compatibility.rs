//! The compatibility fixtures (`tests/compatibility`): Botwork reads every
//! version it supports, refuses newer ones saying what to do, deprecates with a
//! notice, and keeps the formats it writes in the shape of their version.
use botwork::core::{
    packages::Lock,
    report::{RunRecord, RECORD_VERSION},
    worker::protocol::{self, WorkerProtocol},
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/compatibility")
}

fn copy(from: &Path, to: &Path) {
    if from.is_dir() {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            copy(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        fs::copy(from, to).unwrap();
    }
}

/// A scratch directory the CLI runs in, with a package cache of its own.
struct Workspace(tempfile::TempDir);

impl Workspace {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    /// Copy the fixture `name` into the workspace, keeping its name.
    fn add(&self, name: &str) {
        let from = fixtures().join(name);
        copy(&from, &self.path().join(from.file_name().unwrap()));
    }

    fn with(self, name: &str) -> Self {
        self.add(name);
        self
    }

    fn botwork(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_botwork"))
            .args(arguments)
            .current_dir(self.path())
            .env("BOTWORK_CACHE_DIR", self.path().join(".cache"))
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn read(&self, name: &str) -> Vec<u8> {
        fs::read(self.path().join(name)).unwrap()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn failed_case_records_are_read_at_supported_versions_and_refused_when_newer() {
    let workspace = Workspace::new().with("outputs/compat.suite.botwork");
    for version in ["v1.json", "v2.json", "v3.json"] {
        workspace.add(&format!("failed-cases/{version}"));
    }
    let rerun = |record: &str, more: &[&str]| {
        let mut arguments = vec!["--suite", "compat.suite.botwork", "--rerun-failed", record];
        arguments.extend(more);
        workspace.botwork(&arguments)
    };
    // The current version selects its failure, quietly.
    let output = rerun("v2.json", &[]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "the selected case fails again"
    );
    let text = stderr(&output);
    assert!(
        text.contains("[cases] 1 selected: 0 succeeded, 1 failed"),
        "{text}"
    );
    assert!(!text.contains("[deprecated]"), "{text}");
    // Version 1 selects it too, with a deprecation that leaves the outcome alone.
    let output = rerun("v1.json", &[]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(
        text.contains("[cases] 1 selected: 0 succeeded, 1 failed"),
        "{text}"
    );
    assert!(
        text.contains("[deprecated] failed-case record v1.json is version 1, which a later Botwork will stop reading; rewrite it as version 2 by also passing --failures v1.json\n"),
        "{text}"
    );
    // Following the notice upgrades the record to what Botwork writes now.
    let output = rerun("v1.json", &["--failures", "v1.json"]);
    assert!(stderr(&output).contains("[deprecated]"));
    assert_eq!(
        workspace.read("v1.json"),
        fs::read(fixtures().join("failed-cases/v2.json")).unwrap()
    );
    assert!(!stderr(&rerun("v1.json", &[])).contains("[deprecated]"));
    // A newer record is neither read nor replaced, and nothing runs.
    let newer = workspace.read("v3.json");
    for arguments in [
        &[
            "--suite",
            "compat.suite.botwork",
            "--rerun-failed",
            "v3.json",
        ][..],
        &["--suite", "compat.suite.botwork", "--failures", "v3.json"][..],
    ] {
        let output = workspace.botwork(arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
        let text = stderr(&output);
        assert!(text.contains("[BW7002]"), "{text}");
        assert!(
            text.contains("v3.json is version 3, from a newer Botwork; this one reads versions 1 and 2. Upgrade Botwork to use it"),
            "{text}"
        );
        assert_eq!(workspace.read("v3.json"), newer);
    }
}

#[test]
fn lockfiles_are_read_at_their_version_and_newer_ones_say_how_to_lock_again() {
    let workspace = Workspace::new().with("packages/locked");
    let locked = workspace.read("locked/botwork.lock");
    assert!(String::from_utf8_lossy(&locked).contains(&format!("version = {}\n", Lock::VERSION)));
    let run = || workspace.botwork(&["--file", "locked/main.botwork"]);
    let output = run();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Hello, Ada\n");
    let output = workspace.botwork(&["--fetch", "locked", "--locked"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(workspace.read("locked/botwork.lock"), locked);

    copy(
        &fixtures().join("packages/newer.lock"),
        &workspace.path().join("locked/botwork.lock"),
    );
    let newer = workspace.read("locked/botwork.lock");
    for output in [
        run(),
        workspace.botwork(&["--fetch", "locked"]),
        workspace.botwork(&["--fetch", "locked", "--offline"]),
    ] {
        assert_eq!(output.status.code(), Some(1));
        assert!(
            output.stdout.is_empty() || !String::from_utf8_lossy(&output.stdout).contains("Hello")
        );
        let text = stderr(&output);
        assert!(
            text.contains("lockfile version 2 is from a newer Botwork; this one reads version 1. Upgrade Botwork, or delete botwork.lock and run `botwork --fetch` to lock the project again"),
            "{text}"
        );
        assert_eq!(workspace.read("locked/botwork.lock"), newer);
    }
    // Following the message locks the project again, as this Botwork writes it.
    fs::remove_file(workspace.path().join("locked/botwork.lock")).unwrap();
    let output = workspace.botwork(&["--fetch", "locked"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(workspace.read("locked/botwork.lock"), locked);
    assert_eq!(String::from_utf8_lossy(&run().stdout), "Hello, Ada\n");
}

#[test]
fn projects_and_packages_needing_another_botwork_say_so_before_running() {
    let workspace = Workspace::new().with("packages");
    let current = env!("CARGO_PKG_VERSION");
    for (project, message) in [
        (
            "project-needs-newer",
            format!("`needs-newer` 1.0.0 needs Botwork >=99, but this is Botwork {current}; upgrade Botwork"),
        ),
        (
            "newer-keys",
            format!("botwork.toml: it needs Botwork >=99, but this is Botwork {current}; upgrade Botwork to read it"),
        ),
        (
            "dependency-needs-newer",
            format!("`helpers` 1.0.0 needs Botwork >=99, but this is Botwork {current}; upgrade Botwork, or depend on a version of `helpers` that supports Botwork {current} and run `botwork --fetch`"),
        ),
    ] {
        let main = format!("packages/{project}/main.botwork");
        let directory = format!("packages/{project}");
        for arguments in [
            &["--file", main.as_str()][..],
            &["--check", "--file", main.as_str()][..],
            &["--fetch", directory.as_str()][..],
        ] {
            let output = workspace.botwork(arguments);
            assert_eq!(output.status.code(), Some(1), "{arguments:?}");
            assert!(!String::from_utf8_lossy(&output.stdout).contains("Hello"));
            let text = stderr(&output);
            assert!(text.contains(&message), "{arguments:?}: {text}");
        }
    }
}

#[test]
fn report_journals_reconcile_at_their_version_and_newer_ones_are_left_alone() {
    let current = Workspace::new();
    copy(&fixtures().join("journals/v1"), current.path());
    let output = current.botwork(&["--reconcile-report", "report.json"]);
    assert_eq!(output.status.code(), Some(1), "an interrupted run fails");
    assert!(
        stderr(&output).contains(
            "Reconciled report.json: 2 of 2 selected runs have records, 1 of them interrupted"
        ),
        "{}",
        stderr(&output)
    );
    let report: Value = serde_json::from_slice(&current.read("report.json")).unwrap();
    assert_eq!(report["complete"], true);
    assert_eq!(report["verdict"]["status"], "interrupted");
    let statuses: Vec<_> = report["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            (
                run["identity"]["id"].as_str().unwrap(),
                run["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            ("interrupted/finished", "succeeded"),
            ("interrupted/killed", "interrupted")
        ]
    );
    assert!(!current.path().join("report.json.journal").exists());

    let newer = Workspace::new();
    copy(&fixtures().join("journals/v2"), newer.path());
    let before = (newer.read("report.json"), newer.read("report.json.journal"));
    let output = newer.botwork(&["--reconcile-report", "report.json"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(text.contains("[BW4001]"), "{text}");
    assert!(
        text.contains("the journal is version 2, from a newer Botwork; this one reads version 1. Reconcile it with the Botwork that wrote it"),
        "{text}"
    );
    assert_eq!(
        (newer.read("report.json"), newer.read("report.json.journal")),
        before
    );
}

#[test]
fn run_records_read_back_exactly_at_their_version_and_newer_ones_are_refused() {
    let read = |name: &str| fs::read_to_string(fixtures().join("run-records").join(name)).unwrap();
    let current = read(&format!("v{RECORD_VERSION}.json"));
    let record: RunRecord = serde_json::from_str(&current).unwrap();
    assert_eq!(record.identity.id, "compat/fails");
    assert_eq!(
        serde_json::to_value(&record).unwrap(),
        serde_json::from_str::<Value>(&current).unwrap(),
        "a record reads back as written"
    );
    let error = serde_json::from_str::<RunRecord>(&read("v2.json"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("botwork-run record version 2 is from a newer Botwork; this one reads version 1. Upgrade Botwork to read it"),
        "{error}"
    );
}

#[test]
fn worker_frames_decode_at_their_version_and_name_both_versions_otherwise() {
    let read = |name: &str| fs::read(fixtures().join("worker").join(name)).unwrap();
    let protocol = WorkerProtocol::default();
    let value = protocol
        .decode_response(&read(&format!("response-v{}.bin", protocol::VERSION)))
        .unwrap()
        .unwrap();
    assert_eq!(value.to_string(), "hello");
    let error = protocol
        .decode_response(&read("response-v2.bin"))
        .unwrap_err();
    assert_eq!(error.code().as_str(), "BW5003");
    assert!(
        error
            .to_string()
            .contains("Unsupported worker protocol version 2; this side speaks version 1"),
        "{error}"
    );
}

#[cfg(feature = "wasm")]
#[test]
fn components_for_another_interface_version_are_refused_naming_both() {
    let workspace = Workspace::new().with("wasm/statements-0.2.0.wasm");
    fs::write(
        workspace.path().join("main.botwork"),
        "Import |\"statements-0.2.0.wasm\"| As |statements|\n",
    )
    .unwrap();
    let output = workspace.botwork(&["--file", "main.botwork"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(text.contains("[BW6001]"), "{text}");
    assert!(
        text.contains("the component implements botwork:statements/statements@0.2.0, but this Botwork supports 0.1.x; rebuild it against this Botwork's wit/botwork.wit (0.1.0), or use a Botwork that supports 0.2.0"),
        "{text}"
    );
}

/// Every path in `value`, with its JSON type; array items share `[]`, and an
/// event stream's paths start with each event's kind.
fn shape(value: &Value, path: &str, paths: &mut BTreeSet<String>) {
    let kind = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(items) => {
            for item in items {
                shape(item, &format!("{path}[]"), paths);
            }
            "array"
        }
        Value::Object(fields) => {
            for (name, field) in fields {
                shape(field, &format!("{path}.{name}"), paths);
            }
            "object"
        }
    };
    paths.insert(format!("{path}: {kind}"));
}

#[cfg(unix)]
fn events(text: &str) -> (u64, BTreeSet<String>) {
    let mut paths = BTreeSet::new();
    let mut version = None;
    for line in text.lines() {
        let event: Value = serde_json::from_str(line).unwrap();
        version = version.or_else(|| event["version"].as_u64());
        shape(&event, event["event"].as_str().unwrap(), &mut paths);
    }
    (version.expect("the first event names the version"), paths)
}

#[test]
fn written_formats_keep_the_shape_of_their_version() {
    let workspace = Workspace::new().with("outputs/compat.suite.botwork");
    #[allow(unused_mut)]
    let mut arguments = vec![
        "--suite",
        "compat.suite.botwork",
        "--report-json",
        "report.json",
        "--failures",
        "failed.json",
    ];
    #[cfg(unix)]
    arguments.extend([
        "--listener",
        "sh",
        "--listener-arg",
        "-c",
        "--listener-arg",
        "cat > events.jsonl",
    ]);
    let output = workspace.botwork(&arguments);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    // The report's schema is closed: exactly its version's fields.
    let report: Value = serde_json::from_slice(&workspace.read("report.json")).unwrap();
    let version = report["version"].as_u64().unwrap();
    let fixture = fixtures().join(format!("outputs/report-v{version}.json"));
    assert!(
        fixture.is_file(),
        "report version {version} needs its fixture, {}",
        fixture.display()
    );
    let fixture: Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    let (mut written, mut expected) = (BTreeSet::new(), BTreeSet::new());
    shape(&report, "", &mut written);
    shape(&fixture, "", &mut expected);
    assert_eq!(
        written.symmetric_difference(&expected).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "the report changed shape without a new version"
    );

    // Failed-case records have exactly their version's fields too.
    let failed: Value = serde_json::from_slice(&workspace.read("failed.json")).unwrap();
    let version = failed["version"].as_u64().unwrap();
    assert_eq!(
        workspace.read("failed.json"),
        fs::read(fixtures().join(format!("failed-cases/v{version}.json"))).unwrap()
    );

    // Consumers ignore fields added to the event stream, so the stream keeps
    // at least its version's fields, with their types.
    #[cfg(unix)]
    {
        let (version, written) =
            events(&String::from_utf8(workspace.read("events.jsonl")).unwrap());
        let fixture = fixtures().join(format!("outputs/events-v{version}.jsonl"));
        assert!(
            fixture.is_file(),
            "event stream version {version} needs its fixture"
        );
        let (_, expected) = events(&fs::read_to_string(fixture).unwrap());
        assert_eq!(
            expected.difference(&written).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "the event stream lost or retyped fields without a new version"
        );
    }
}

/// The rows of the versioned-contracts table in `docs/compatibility.md`.
fn contracts() -> Vec<Vec<String>> {
    let page =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/compatibility.md"))
            .unwrap();
    let section = page
        .split("## Versioned contracts")
        .nth(1)
        .and_then(|rest| rest.split("\n#").next())
        .expect("a Versioned contracts section");
    section
        .lines()
        .filter(|line| {
            line.starts_with("| ") && !line.starts_with("| Contract") && !line.starts_with("| ---")
        })
        .map(|line| {
            line.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect()
        })
        .collect()
}

#[test]
fn the_versioned_contracts_table_matches_the_fixtures_and_the_code() {
    let rows = contracts();
    assert!(rows.iter().all(|row| row.len() == 5), "{rows:?}");
    // Every fixture the table names exists, and every fixture is named.
    let named: Vec<PathBuf> = rows
        .iter()
        .flat_map(|row| row[4].split(", ").filter(|cell| !cell.is_empty()))
        .map(|cell| fixtures().join(cell.trim_matches('`')))
        .collect();
    for path in &named {
        assert!(path.exists(), "{} is named but missing", path.display());
    }
    fn files(directory: &Path, into: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, into);
            } else {
                into.push(path);
            }
        }
    }
    let mut all = Vec::new();
    files(&fixtures(), &mut all);
    for path in all {
        let name = path.file_name().unwrap().to_string_lossy();
        let source = name == "README.md" || name.ends_with(".suite.botwork");
        assert!(
            source || named.iter().any(|named| path.starts_with(named)),
            "{} is in no row of the versioned-contracts table",
            path.display()
        );
    }
    // The current versions are the code's.
    let current = |label: &str| {
        rows.iter()
            .find(|row| row[0].starts_with(label))
            .unwrap_or_else(|| panic!("a row for {label}"))[1]
            .clone()
    };
    let wit =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/botwork.wit")).unwrap();
    let interface = wit
        .lines()
        .find_map(|line| {
            line.strip_prefix("package botwork:statements@")?
                .strip_suffix(';')
        })
        .unwrap();
    for (label, version) in [
        ("Botwork", env!("CARGO_PKG_VERSION").to_owned()),
        ("[Lockfile]", Lock::VERSION.to_string()),
        ("[Run record]", RECORD_VERSION.to_string()),
        ("[Worker protocol]", protocol::VERSION.to_string()),
        ("[WebAssembly interface]", interface.to_owned()),
    ] {
        assert_eq!(current(label), version, "{label}");
    }
    // The written formats' versions are their fixtures' newest.
    for (label, prefix) in [
        ("[JSON report]", "outputs/report-v"),
        ("[Event stream]", "outputs/events-v"),
    ] {
        let newest = named
            .iter()
            .filter_map(|path| {
                let relative = path
                    .strip_prefix(fixtures())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                relative
                    .strip_prefix(prefix)?
                    .split('.')
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap();
        assert_eq!(current(label), newest.to_string(), "{label}");
    }
}
