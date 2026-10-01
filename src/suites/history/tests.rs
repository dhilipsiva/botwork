use super::*;
use crate::atomic_file::TEMPORARY;
use std::{fs, sync::atomic::Ordering};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "botwork-suite-history-{}-{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn writer_admits_exact_id_capacity_and_rejections_preserve_the_previous_record() {
    let _serial = SERIAL.lock().unwrap();
    let directory = Directory::new();
    let path = directory.0.join("failed.json");
    let history = History::begin(path.clone()).unwrap();
    assert!(load(&path).is_err());
    let exact = (0..suite::MAX_SELECTED_CASES)
        .map(|i| format!("{}/{:0>128}/{}", "s".repeat(128), i, "r".repeat(128)))
        .collect::<Vec<_>>();
    history.finish(exact.clone()).unwrap();
    assert_eq!(load(&path).unwrap().0, exact);
    let mut over = exact.clone();
    over.push("suite/over".into());
    for invalid in [
        over,
        vec!["suite/a".into(), "suite/a".into()],
        vec!["suite/".into()],
        vec!["suite/case/row/extra".into()],
    ] {
        assert!(history.finish(invalid).is_err());
        assert_eq!(load(&path).unwrap().0, exact);
    }
    history.finish(vec![]).unwrap();
    assert!(load(&path).unwrap().0.is_empty());
}

#[test]
fn temporary_collisions_are_retried_without_overwrite_and_other_io_errors_keep_their_cause() {
    let _serial = SERIAL.lock().unwrap();
    let directory = Directory::new();
    let path = directory.0.join("failed.json");
    let history = History::begin(path.clone()).unwrap();
    let collision = directory.0.join(format!(
        ".failed.json.{}.{}.tmp",
        std::process::id(),
        TEMPORARY.load(Ordering::Relaxed)
    ));
    fs::write(&collision, "unrelated temporary file").unwrap();
    history.finish(vec!["s/a".into()]).unwrap();
    assert_eq!(load(&path).unwrap().0, ["s/a"]);
    assert_eq!(
        fs::read_to_string(&collision).unwrap(),
        "unrelated temporary file"
    );
    // Windows cannot move a directory while the record's lock is open in it.
    #[cfg(unix)]
    {
        let moved = Directory(directory.0.with_extension("moved"));
        fs::rename(&directory.0, &moved.0).unwrap();
        let error = history.finish(vec![]).unwrap_err().to_string();
        assert!(error.contains("Failed-case record"), "{error}");
        assert!(!error.contains("collisions exhausted"), "{error}");
        assert_eq!(load(&moved.0.join("failed.json")).unwrap().0, ["s/a"]);
    }
}

#[test]
fn version_one_reads_with_a_deprecation_and_newer_versions_are_refused_by_version() {
    let _serial = SERIAL.lock().unwrap();
    let directory = Directory::new();
    let path = directory.0.join("failed.json");
    let write = |record: serde_json::Value| fs::write(&path, record.to_string()).unwrap();
    write(serde_json::json!({"format": FORMAT, "version": 2, "complete": true, "failed": ["s/a"]}));
    assert_eq!(load(&path).unwrap(), (vec!["s/a".to_owned()], None));
    write(serde_json::json!({"format": FORMAT, "version": 1, "complete": true, "failed": ["s/a"]}));
    let (failed, deprecated) = load(&path).unwrap();
    assert_eq!(failed, ["s/a"]);
    let deprecated = deprecated.expect("version 1 is deprecated");
    assert!(
        deprecated.contains("is version 1, which a later Botwork will stop reading; rewrite it as version 2 by also passing --failures"),
        "{deprecated}"
    );
    // A newer record's new fields are reported as its version, and it is
    // neither read nor replaced.
    for version in [3, 300] {
        write(
            serde_json::json!({"format": FORMAT, "version": version, "complete": true, "failed": [], "added": true}),
        );
        for error in [
            load(&path).unwrap_err().to_string(),
            History::begin(path.clone()).err().unwrap().to_string(),
        ] {
            assert!(
                error.contains(&format!("is version {version}, from a newer Botwork; this one reads versions 1 and 2. Upgrade Botwork")),
                "{error}"
            );
        }
        assert!(fs::read_to_string(&path).unwrap().contains("added"));
    }
    write(serde_json::json!({"format": FORMAT, "version": 0, "complete": true, "failed": []}));
    assert!(load(&path)
        .unwrap_err()
        .to_string()
        .contains("Unsupported or inconsistent failed-case record"));
}
