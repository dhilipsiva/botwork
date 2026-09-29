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
    assert_eq!(load(&path).unwrap(), exact);
    let mut over = exact.clone();
    over.push("suite/over".into());
    for invalid in [
        over,
        vec!["suite/a".into(), "suite/a".into()],
        vec!["suite/".into()],
        vec!["suite/case/row/extra".into()],
    ] {
        assert!(history.finish(invalid).is_err());
        assert_eq!(load(&path).unwrap(), exact);
    }
    history.finish(vec![]).unwrap();
    assert!(load(&path).unwrap().is_empty());
}

#[test]
fn temporary_collisions_are_retried_without_overwrite_and_other_io_errors_keep_their_cause() {
    let _serial = SERIAL.lock().unwrap();
    let mut directory = Directory::new();
    let path = directory.0.join("failed.json");
    let history = History::begin(path.clone()).unwrap();
    let collision = directory.0.join(format!(
        ".failed.json.{}.{}.tmp",
        std::process::id(),
        TEMPORARY.load(Ordering::Relaxed)
    ));
    fs::write(&collision, "unrelated temporary file").unwrap();
    history.finish(vec!["s/a".into()]).unwrap();
    assert_eq!(load(&path).unwrap(), ["s/a"]);
    assert_eq!(
        fs::read_to_string(&collision).unwrap(),
        "unrelated temporary file"
    );
    let moved = directory.0.with_extension("moved");
    fs::rename(&directory.0, &moved).unwrap();
    directory.0 = moved;
    let error = history.finish(vec![]).unwrap_err().to_string();
    assert!(error.contains("Failed-case record"), "{error}");
    assert!(!error.contains("collisions exhausted"), "{error}");
    assert_eq!(load(&directory.0.join("failed.json")).unwrap(), ["s/a"]);
}
