//! The usability study kit (`docs/usability-study.md`): each task's reference
//! solution passes its automated check, the starter files and plausible wrong
//! answers fail it for the stated reason, workspaces counterbalance the task
//! order, and `score.py` applies every registered threshold.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const TASKS: [&str; 6] = ["U1", "U2", "U3", "U4", "U5", "U6"];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn python(script: &str, arguments: &[&str]) -> Output {
    Command::new("python3")
        .arg(root().join("usability").join(script))
        .args(arguments)
        .output()
        .expect("run python3")
}

/// A fresh workspace for `participant`, as the facilitator prepares it.
fn workspace(participant: u32) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("workspace");
    let output = python(
        "prepare.py",
        &[&participant.to_string(), folder.to_str().unwrap()],
    );
    assert!(output.status.success(), "{output:?}");
    (directory, folder)
}

fn check(task: &str, folder: &Path) -> Value {
    let output = python(
        "check.py",
        &[
            task,
            folder.join(task).to_str().unwrap(),
            "--botwork",
            env!("CARGO_BIN_EXE_botwork"),
        ],
    );
    let verdict: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{task}: {}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(
        output.status.success(),
        verdict["passed"] == true,
        "{task}: {verdict}"
    );
    verdict
}

fn solve(task: &str, folder: &Path) {
    let solution = root().join("tests/usability/solutions").join(task);
    for entry in fs::read_dir(solution).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, folder.join(task).join(path.file_name().unwrap())).unwrap();
    }
}

/// Require a failed check with a reason containing `reason`.
fn fails_because(verdict: &Value, reason: &str) {
    assert_eq!(verdict["passed"], false, "{verdict}");
    assert!(
        verdict["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|found| found.as_str().unwrap().contains(reason)),
        "expected a reason containing {reason:?}: {verdict}"
    );
}

#[test]
fn reference_solutions_pass_their_checks() {
    let (_directory, folder) = workspace(1);
    for task in TASKS {
        solve(task, &folder);
        let verdict = check(task, &folder);
        assert_eq!(verdict["passed"], true, "{task}: {verdict}");
    }
}

#[test]
fn untouched_starter_files_fail_their_checks() {
    let (_directory, folder) = workspace(2);
    for (task, reason) in [
        ("U1", "answer.txt is missing"),
        ("U2", "first.botwork is missing"),
        ("U3", "multiplies 0 times"),
        ("U4", "ran 0 times"),
        ("U5", "expected 1 for a failure"),
        ("U6", "misspelled reference in pricing.botwork is unchanged"),
    ] {
        fails_because(&check(task, &folder), reason);
    }
}

#[test]
fn the_reading_task_prints_the_answer_its_check_expects() {
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--file", "order.botwork"])
        .current_dir(root().join("usability/tasks/U1"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let answer =
        fs::read_to_string(root().join("tests/usability/solutions/U1/answer.txt")).unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), answer);
    assert_eq!(answer, "12\nlow\n14\nhigh\n0\nlow\n99\n");
}

#[test]
fn plausible_wrong_answers_fail_for_the_stated_reason() {
    let pricing = fs::read_to_string(root().join("usability/tasks/U6/pricing.botwork")).unwrap();
    let suite =
        fs::read_to_string(root().join("tests/usability/solutions/U5/totals.suite.botwork"))
            .unwrap();
    let cases: &[(&str, &str, String, &str)] = &[
        ("U1", "answer.txt", "12\nlow\n14\nhigh\n0\nlow\n3\n".into(), "predicted"),
        ("U2", "first.botwork", "|quantity| = |3|\n|price| = |4|\nLog |12|\n".into(), "not calculated"),
        ("U2", "first.botwork", "|quantity| = |3|\n|price| = |5|\nLog |quantity * price|\n".into(), "price is not set to 4"),
        (
            "U3",
            "reuse.botwork",
            "|quantity| = |99|\nLog |3 * 4|\nLog |2 * 7|\nLog |quantity|\n".into(),
            "multiplies 2 times",
        ),
        (
            "U3",
            "reuse.botwork",
            "|quantity| = |99|\nTotal |price| {\n    Return |quantity * price|\n}\n|quantity| = |3|\nLog |@{ Total |4| }|\n|quantity| = |2|\nLog |@{ Total |7| }|\nLog |quantity|\n".into(),
            "printed",
        ),
        (
            "U4",
            "compose.botwork",
            "Import |\"pricing.botwork\"| As |pricing|\nLog |3 * 4 + 2 * 7 == 26|\n".into(),
            "multiplies itself",
        ),
        (
            "U4",
            "pricing.botwork",
            "Total of |quantity| at |price| {\n    Return |13|\n}\n".into(),
            "pricing.botwork was changed",
        ),
        (
            "U4",
            "compose.botwork",
            "Import |\"pricing.botwork\"| As |pricing|\n|first| = |@{ pricing::Total of |3| at |4| }|\n|first| = |@{ pricing::Total of |3| at |4| }|\n|second| = |@{ pricing::Total of |2| at |7| }|\nLog |first + second == 26|\n".into(),
            "ran 3 times",
        ),
        (
            "U5",
            "totals.suite.botwork",
            suite.replace("expected: 99", "expected: 14"),
            "expected two rows to pass and one to fail",
        ),
        (
            "U5",
            "totals.suite.botwork",
            suite
                .replace("quantity: 3, price: 4, expected: 12", "quantity: 3, price: 4, expected: 13")
                .replace("expected: 99", "expected: 14"),
            "not the 2 x 7 row",
        ),
        ("U5", "rerun.args", "--suite totals.suite.botwork\n".into(), "the rerun selected"),
        (
            "U6",
            "pricing.botwork",
            pricing.replace("less |discount|", "less |discont|"),
            "more than the misspelled reference",
        ),
        (
            "U6",
            "main.botwork",
            "Import |\"pricing.botwork\"| As |pricing|\n\n|subtotal| = |@{ pricing::Line total of |3| at |4| less |2| }|\nLog |subtotal|\n".into(),
            "main.botwork was changed",
        ),
    ];
    for (task, file, content, reason) in cases {
        let (_directory, folder) = workspace(3);
        solve(task, &folder);
        fs::write(folder.join(task).join(file), content).unwrap();
        fails_because(&check(task, &folder), reason);
    }
}

#[test]
fn workspaces_counterbalance_the_task_order() {
    let orders: Vec<Vec<String>> = (1..=7)
        .map(|participant| {
            let (_directory, folder) = workspace(participant);
            let order: Vec<String> = fs::read_to_string(folder.join("order.txt"))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect();
            let kit: Value =
                serde_json::from_str(&fs::read_to_string(folder.join("kit.json")).unwrap())
                    .unwrap();
            assert_eq!(kit["order"], serde_json::json!(order));
            assert_eq!(kit["files"].as_object().unwrap().len(), 13);
            for task in TASKS {
                assert!(folder.join(task).join("prompt.md").is_file());
            }
            order
        })
        .collect();
    // Six distinct orders, then the rotation repeats.
    assert_eq!(orders[6], orders[0]);
    let distinct: std::collections::BTreeSet<_> = orders[..6].iter().collect();
    assert_eq!(distinct.len(), 6);
    for order in &orders {
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, TASKS);
        // U2, the first script, comes before the other writing tasks.
        assert_eq!(order[1], "U2");
        assert!(["U1", "U6"].contains(&order[0].as_str()));
    }
    // Each of U3 to U5 takes each middle position equally often.
    for task in ["U3", "U4", "U5"] {
        for position in 2..5 {
            let count = orders[..6]
                .iter()
                .filter(|order| order[position] == task)
                .count();
            assert_eq!(count, 2, "{task} at position {position}");
        }
    }
}

/// A sheet's rows: every participant succeeds unaided at every task in
/// `seconds`, except where `change` rewrites a row.
fn sheet(
    newcomers: usize,
    experienced: usize,
    change: impl Fn(usize, &str, &mut [String; 10]),
) -> String {
    let mut text = String::from(
        "participant,cohort,round,task,seconds,outcome,assisted,checked,explanation_ok,notes\n",
    );
    for index in 0..newcomers + experienced {
        let cohort = if index < newcomers {
            "newcomer"
        } else {
            "experienced"
        };
        for task in TASKS {
            let mut row = [
                format!("P{:02}", index + 1),
                cohort.into(),
                "original".into(),
                task.into(),
                "240".into(),
                "success".into(),
                "no".into(),
                "pass".into(),
                if task == "U1" {
                    "yes".into()
                } else {
                    String::new()
                },
                String::new(),
            ];
            change(index, task, &mut row);
            text.push_str(&row.join(","));
            text.push('\n');
        }
    }
    text
}

fn score(text: &str) -> (i32, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sheet.csv");
    fs::write(&path, text).unwrap();
    let output = python("score.py", &[path.to_str().unwrap()]);
    (
        output.status.code().unwrap(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
    )
}

fn failing(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter_map(|line| line.trim().strip_prefix("FAIL: "))
        .collect()
}

#[test]
fn scoring_applies_every_registered_threshold() {
    let (status, report) = score(&sheet(5, 5, |_, _, _| {}));
    assert_eq!((status, failing(&report)), (0, vec![]), "{report}");
    assert!(
        report.contains("U3: 10/10 unaided (newcomer 5/5, experienced 5/5)"),
        "{report}"
    );

    // Nine of ten is exactly 90%; eight of ten is not.
    let (status, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U3" && i == 0 {
            row[5] = "failure".into();
            row[7] = "fail".into();
        }
    }));
    assert_eq!(status, 0, "{report}");
    let (status, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U3" && i < 2 {
            row[5] = "abandoned".into();
            row[7] = "fail".into();
        }
    }));
    assert_eq!(status, 1);
    assert_eq!(
        failing(&report),
        ["U3: at least 90% unaided success (0.8)"],
        "{report}"
    );

    // Assisted successes and wrong U1 explanations never count as unaided.
    let (_, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U4" && i < 2 {
            row[6] = "yes".into();
        }
        if task == "U1" && i < 2 {
            row[8] = "no".into();
        }
    }));
    assert_eq!(
        failing(&report),
        [
            "U1: at least 90% unaided success (0.8)",
            "U4: at least 90% unaided success (0.8)"
        ],
        "{report}"
    );

    // A median of exactly 10 minutes passes. Counted at the 20-minute limit,
    // one unsuccessful 100-second attempt raises the median past it.
    let (status, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U2" {
            row[4] = if i < 6 { "600" } else { "100" }.into();
        }
    }));
    assert_eq!(status, 0, "{report}");
    let (_, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U2" && i < 6 {
            row[4] = if i == 0 { "100" } else { "601" }.into();
            if i == 0 {
                row[5] = "failure".into();
                row[7] = "fail".into();
            }
        }
    }));
    assert!(
        failing(&report)
            .contains(&"U2: median time to a first passing script at most 600 s (601.0)"),
        "{report}"
    );

    // U6 within five minutes: eight of ten passes, seven does not.
    let (status, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U6" {
            row[4] = if i < 8 { "300" } else { "301" }.into();
        }
    }));
    assert_eq!(status, 0, "{report}");
    let (_, report) = score(&sheet(5, 5, |i, task, row| {
        if task == "U6" && i >= 7 {
            row[4] = "301".into();
        }
    }));
    assert_eq!(
        failing(&report),
        ["U6: at least 80% of participants repair within 300 s (0.7)"],
        "{report}"
    );

    // Too few participants, or too few in a cohort.
    for (newcomers, experienced) in [(6, 4), (4, 5)] {
        let (status, report) = score(&sheet(newcomers, experienced, |_, _, _| {}));
        assert_eq!(status, 1, "{report}");
        assert!(
            failing(&report)[0].starts_with("at least 10 participants"),
            "{report}"
        );
    }
}

#[test]
fn scoring_keeps_retests_apart_and_rejects_invalid_sheets() {
    let mut text = sheet(5, 5, |_, _, _| {});
    // A failed retest by a fresh participant neither fails nor replaces the round.
    for task in TASKS {
        text.push_str(&format!(
            "P11,newcomer,retest,{task},1200,timeout,no,fail,{},\n",
            if task == "U1" { "no" } else { "" }
        ));
    }
    let (status, report) = score(&text);
    assert_eq!(status, 0, "{report}");
    assert!(report.contains("retest round"), "{report}");
    assert!(report.contains("U3: 0/1 unaided"), "{report}");

    let valid = sheet(5, 5, |_, _, _| {});
    for (invalid, reason) in [
        (
            valid.replacen("participant,", "person,", 1),
            "columns must be",
        ),
        (valid.replacen(",240,", ",1201,", 1), "task limit"),
        (
            valid.replacen(",success,no,pass,", ",success,no,fail,", 1),
            "must pass its automated check",
        ),
        (
            valid.replacen(",newcomer,", ",novice,", 1),
            "cohort must be",
        ),
        (
            valid.replacen("U1,240,success,no,pass,yes", "U1,240,success,no,pass,", 1),
            "explanation_ok",
        ),
        (
            format!("{valid}P01,newcomer,original,U2,240,success,no,pass,,\n"),
            "more than one U2 row",
        ),
    ] {
        let (status, report) = score(&invalid);
        assert_eq!(status, 2, "{reason}: {report}");
        assert!(report.contains(reason), "{reason}: {report}");
    }
    let missing: String = valid
        .lines()
        .filter(|line| !line.starts_with("P03,newcomer,original,U5"))
        .map(|line| format!("{line}\n"))
        .collect();
    let (status, report) = score(&missing);
    assert_eq!(status, 1, "{report}");
    assert!(report.contains("'missing': ['P03/U5']"), "{report}");
}
