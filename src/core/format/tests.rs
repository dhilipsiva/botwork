use super::*;
use crate::core::diagnostic::DiagnosticCode;

fn script(source: &str) -> String {
    format("test.botwork", source, SourceKind::Script).unwrap()
}

#[test]
fn statements_get_canonical_keywords_indentation_and_spacing() {
    assert_eq!(
        script("log|\"a\"|\nif|x>1|{Log|x|}else if |x==1| {log |1|}else{}\nfor |i| in |[1,2]| { |x|=|x+i| }\nwhile|true|{break}\n"),
        "log |\"a\"|\nIf |x > 1| {\n    Log |x|\n} Else If |x == 1| {\n    log |1|\n} Else {}\nFor |i| In |[1, 2]| {\n    |x| = |x + i|\n}\nWhile |true| {\n    Break\n}\n"
    );
    assert_eq!(
        script(
            "What   is  the |a|   plus|b|   { Return |a+b| }\nLog |@{ what is the |1|plus |2| }|\n"
        ),
        "What is the |a| plus |b| {\n    Return |a + b|\n}\nLog |@{ what is the |1| plus |2| }|\n"
    );
    assert_eq!(
        script("Log |-2 ^ 2 + (3*4) - !true|\nLog |@{Double|2|}.value[0]|\nImport|\"m.botwork\"|as|m|\nretry|{attempts:2}|{Log|1|}\n"),
        "Log |-2 ^ 2 + (3 * 4) - !true|\nLog |@{ Double |2| }.value[0]|\nImport |\"m.botwork\"| As |m|\nRetry |{attempts: 2}| {\n    Log |1|\n}\n"
    );
}

#[test]
fn comments_keep_their_place_and_inner_comments_move_above_their_statement() {
    let source = "# top\n\n\n|x| = |1|   # trailing\nIf |x| { # opening\n    # inside\n\n\n    Log |x| # after\n    # last\n}\nTry { Fail |\"x\"| } Catch |e| { Log |e.code| } Finally { Log |1| }\nOuter { }\nEmpty {\n    # only a comment\n}\nLog |[1, ### block ### 2]|\n";
    assert_eq!(
        script(source),
        "# top\n\n|x| = |1| # trailing\nIf |x| { # opening\n    # inside\n\n    Log |x| # after\n    # last\n}\nTry {\n    Fail |\"x\"|\n} Catch |e| {\n    Log |e.code|\n} Finally {\n    Log |1|\n}\nOuter {}\nEmpty {\n    # only a comment\n}\n### block ###\nLog |[1, 2]|\n"
    );
}

#[test]
fn collections_written_across_lines_stay_one_item_per_line() {
    assert_eq!(
        script("|x| = |[1,\n  2, # two\n  3]|\n|m| = |{a: 1,\n \"b c\": [1,\n 2]}|\n|flat| = |[ 1 ,2 ]|\n|empty| = |[\n]|\n"),
        "# two\n|x| = |[\n    1,\n    2,\n    3,\n]|\n|m| = |{\n    a: 1,\n    \"b c\": [\n        1,\n        2,\n    ],\n}|\n|flat| = |[1, 2]|\n|empty| = |[]|\n"
    );
}

#[test]
fn literals_are_kept_and_line_endings_become_lf() {
    assert_eq!(
        script("Log |\"keep  \\\"spaces\\\"\r\n  inside\"|\r\nLog |0.50|\r\n"),
        "Log |\"keep  \\\"spaces\\\"\r\n  inside\"|\nLog |0.50|\n"
    );
    assert_eq!(script(""), "");
    assert_eq!(script("\n\n# only\n\n"), "# only\n");
}

#[test]
fn long_sentences_wrap_between_words_when_every_piece_fits() {
    let long = "A very long statement name that keeps going and going with |first| and more words |second| and then |third| and |fourth| end\n";
    let wrapped = script(long);
    assert_eq!(
        wrapped,
        "A very long statement name that keeps going and going with |first| and more words |second| \\\n    and then |third| and |fourth| end\n"
    );
    assert!(wrapped
        .lines()
        .all(|line| line.chars().count() <= MAX_WIDTH));
    // A parameter too long for any line is left on one line.
    let value = "x".repeat(MAX_WIDTH);
    let single = format!("Log |\"{value}\"|\n");
    assert_eq!(script(&single), single);
}

#[test]
fn suites_and_datasets_are_laid_out_like_scripts() {
    let suite = "Suite|\"s\"|named |\"S\"| tags|[\"a\",\"b\"]|{\ndataset |\"d\"| from json |\"rows.json\"|\n  Dataset|\"i\"|{row|\"one\"|values|{x: -1, y: none}| # one\n}\nlibrary{Double|x|{Return|x*2|}}\nsuitesetup{|s| = |1|}\n\n\ncase|\"c\"|using|\"i\"|as|row|{Log|row.x|}\n}\n";
    assert_eq!(
        format("t.suite.botwork", suite, SourceKind::Suite).unwrap(),
        "Suite |\"s\"| Named |\"S\"| Tags |[\"a\", \"b\"]| {\n    Dataset |\"d\"| From JSON |\"rows.json\"|\n    Dataset |\"i\"| {\n        Row |\"one\"| Values |{x: -1, y: none}| # one\n    }\n    Library {\n        Double |x| {\n            Return |x * 2|\n        }\n    }\n    SuiteSetup {\n        |s| = |1|\n    }\n\n    Case |\"c\"| Using |\"i\"| As |row| {\n        Log |row.x|\n    }\n}\n"
    );
    assert_eq!(
        format(
            "t.dataset.botwork",
            "dataset|\"d\"|{\nrow |\"r\"| values |[1,\n2]|}",
            SourceKind::Dataset
        )
        .unwrap(),
        "Dataset |\"d\"| {\n    Row |\"r\"| Values |[\n        1,\n        2,\n    ]|\n}\n"
    );
}

#[test]
fn invalid_input_is_refused_with_the_runs_diagnostic() {
    let error = format("bad.botwork", "Log |1\n", SourceKind::Script).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Syntax);
    let error = format("bad.botwork", "Break\n", SourceKind::Script).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::InvalidControl);
    assert!(format("bad.suite.botwork", "Log |1|\n", SourceKind::Suite).is_err());
}

#[test]
fn file_names_choose_the_grammar() {
    assert_eq!(
        SourceKind::of_path(Path::new("a/checkout.suite.botwork")),
        SourceKind::Suite
    );
    assert_eq!(
        SourceKind::of_path(Path::new("rows.dataset.botwork")),
        SourceKind::Dataset
    );
    assert_eq!(
        SourceKind::of_path(Path::new("main.botwork")),
        SourceKind::Script
    );
}

#[test]
fn fingerprints_distinguish_programs_that_differ() {
    let shape = |source: &str| fingerprint("t.botwork", source, SourceKind::Script).unwrap();
    assert_eq!(shape("Log |1 + 2|\n"), shape("log   |1+2|\n"));
    for other in [
        "Log |2 + 1|\n",
        "Log |1 - 2|\n",
        "Log |[1 + 2]|\n",
        "Say |1 + 2|\n",
        "Log |\"1 + 2\"|\n",
    ] {
        assert_ne!(shape("Log |1 + 2|\n"), shape(other), "{other}");
    }
    let rows =
        |source: &str| fingerprint("t.dataset.botwork", source, SourceKind::Dataset).unwrap();
    assert_eq!(
        rows("Dataset |\"d\"| { Row |\"r\"| Values |{a: 1, b: 2}| }"),
        rows("Dataset |\"d\"| { Row |\"r\"| Values |{b: 2, a: 1}| }")
    );
}
