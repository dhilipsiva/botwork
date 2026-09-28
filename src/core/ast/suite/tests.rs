use super::*;
use crate::core::{
    diagnostic::DiagnosticCode,
    run::{Engine, RunOptions, RunOutcome},
};

fn parse(source: &str) -> Arc<Suite> {
    Arc::new(Suite::parse("suite.botwork", source).unwrap())
}

fn rejected<T>(result: DiagnosticResult<T>) -> Diagnostic {
    // Do not Debug-format an unexpectedly accepted AST: each span owns its source.
    match result {
        Err(error) => error,
        Ok(_) => panic!("suite validation must reject this input"),
    }
}

#[test]
fn discovery_preserves_names_tags_order_and_original_body_spans() {
    let source = r#"sUiTe |"math"| Named |"கணிதம்"| Tags |["fast", "common"]| {
Library { Double |n| { Return |n * 2| } }
Case |"second"| Named |"Second display"| Tags |["common", "special"]| { |answer| = Double |21| }
Case |"first"| { |answer| = |7| }
}"#;
    let suite = parse(source);
    assert_eq!(suite.metadata().name(), "கணிதம்");
    assert_eq!(suite.metadata().id(), "math");
    assert_eq!(suite.cases().len(), 2);
    assert_eq!(suite.cases()[0].metadata().id(), "second");
    assert_eq!(suite.cases()[1].metadata().id(), "first");
    assert_eq!(
        suite.metadata().tags(),
        &BTreeSet::from(["common".into(), "fast".into()])
    );
    assert_eq!(suite.source().text(), source);
    assert!(suite.program(2).is_none());
    let cases = Selection::default().select(&[suite]).unwrap();
    assert_eq!(
        cases.iter().map(SelectedCase::id).collect::<Vec<_>>(),
        ["math/second", "math/first"]
    );
    assert_eq!(
        cases[0].tags().into_iter().collect::<Vec<_>>(),
        ["common", "fast", "special"]
    );
    assert_eq!(cases[0].case().metadata().name(), "Second display");
    assert_eq!(cases[1].case().metadata().name(), "first");
    assert_eq!(cases[0].case().span().line_column().0, 3);
    assert_eq!(cases[0].program().source.text(), source);
    assert_eq!(cases[0].program().statements.len(), 2);
    let result = Engine::default().run_program(&cases[0].program(), RunOptions::default());
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.variables["answer"].to_string(), "42");
}

#[test]
fn ids_survive_display_name_and_declaration_order_changes() {
    let original = parse("Suite |\"stable\"| { Case |\"a\"| Named |\"Old\"| {} Case |\"b\"| {} }");
    let changed = parse("Suite |\"stable\"| Named |\"New suite\"| { Case |\"b\"| {} Case |\"a\"| Named |\"New\"| {} }");
    let selection = Selection {
        failed: Some(vec!["stable/a".into()]),
        ..Selection::default()
    };
    for suite in [original, changed] {
        assert_eq!(selection.select(&[suite]).unwrap()[0].id(), "stable/a");
    }
}

#[test]
fn discovery_does_not_resolve_imports_or_execute_case_effects() {
    let suite = parse("Suite |\"discover\"| { Library { Import |\"missing.botwork\"| As |module| } Case |\"a\"| { Log |missing| } }");
    let cases = Selection::default().select(&[suite]).unwrap();
    assert_eq!(cases[0].id(), "discover/a");
    assert_eq!(cases[0].program().statements.len(), 2);
}

#[test]
fn library_rejects_runnable_top_level_statements_and_bodies_keep_control_rules() {
    for source in [
        "Suite |\"s\"| { Library { Log |1| } Case |\"a\"| {} }",
        "Suite |\"s\"| { Library { |x| = |1| } Case |\"a\"| {} }",
        "Suite |\"s\"| { Case |\"a\"| { Return |1| } }",
        "Suite |\"s\"| { Case |\"a\"| { Break } }",
    ] {
        assert!(Suite::parse("invalid", source).is_err(), "{source}");
    }
    assert!(Suite::parse("empty", "Suite |\"s\"| {}").is_err());
}

#[test]
fn metadata_is_literal_bounded_and_duplicate_ids_are_rejected() {
    for source in [
        "Suite |\"s\"| { Case |\"a\"| {} Case |\"a\"| {} }",
        "Suite |\"s/a\"| { Case |\"a\"| {} }",
        "Suite |\"\"| { Case |\"a\"| {} }",
        "Suite |\"s\"| { Case |\"அ\"| {} }",
        "Suite |\"s\"| Tags |[\"a\", \"a\"]| { Case |\"a\"| {} }",
        "Suite |\"s\"| Tags |[\"\\n\"]| { Case |\"a\"| {} }",
        "Suite |\"s\"| Tags |[missing]| { Case |\"a\"| {} }",
    ] {
        assert!(Suite::parse("invalid", source).is_err(), "{source}");
    }
    let id = "a".repeat(MAX_ID_BYTES);
    assert!(Suite::parse(
        "exact",
        &format!("Suite |\"{id}\"| {{ Case |\"a\"| {{}} }}")
    )
    .is_ok());
    assert!(Suite::parse(
        "over",
        &format!("Suite |\"{id}a\"| {{ Case |\"a\"| {{}} }}")
    )
    .is_err());
    let many = (0..=MAX_SUITE_CASES)
        .map(|i| format!("Case |\"c{i}\"| {{}}\n"))
        .collect::<String>();
    assert_eq!(
        rejected(Suite::parse(
            "cases",
            &format!("Suite |\"s\"| {{ {many} }}")
        ))
        .code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn ast_admission_counts_all_case_bodies_together() {
    let body = "|x| = |0|\n".repeat(300);
    let cases = (0..128)
        .map(|i| format!("Case |\"c{i}\"| {{ {body} }}\n"))
        .collect::<String>();
    let error = rejected(Suite::parse(
        "aggregate",
        &format!("Suite |\"s\"| {{ {cases} }}"),
    ));
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(error.to_string().contains("AST nodes"), "{error}");
}

#[test]
fn selection_intersects_ids_included_tags_and_prior_failures_then_excludes_tags() {
    let suite = parse("Suite |\"s\"| Tags |[\"all\"]| { Case |\"a\"| Tags |[\"fast\"]| {} Case |\"b\"| Tags |[\"slow\"]| {} Case |\"c\"| Tags |[\"fast\"]| {} }");
    let selection = Selection {
        cases: vec!["s/c".into(), "s/b".into(), "s/a".into()],
        tags: vec!["all".into(), "unmatched".into()],
        exclude_tags: vec!["slow".into()],
        failed: Some(vec!["s/b".into(), "s/c".into()]),
    };
    let selected = selection.select(&[suite]).unwrap();
    assert_eq!(
        selected.iter().map(SelectedCase::id).collect::<Vec<_>>(),
        ["s/c"]
    );
}

#[test]
fn selection_rejects_stale_ids_duplicates_and_empty_matches_but_allows_no_prior_failures() {
    let suite = parse("Suite |\"s\"| { Case |\"a\"| {} }");
    assert!(Selection::default()
        .select(&[Arc::clone(&suite), Arc::clone(&suite)])
        .is_err());
    for selection in [
        Selection {
            cases: vec!["s/missing".into()],
            ..Selection::default()
        },
        Selection {
            failed: Some(vec!["s/removed".into()]),
            ..Selection::default()
        },
        Selection {
            tags: vec!["unmatched".into()],
            ..Selection::default()
        },
    ] {
        assert!(selection.select(&[Arc::clone(&suite)]).is_err());
    }
    assert!(Selection {
        failed: Some(vec![]),
        ..Selection::default()
    }
    .select(&[suite])
    .unwrap()
    .is_empty());
}

#[test]
fn suite_words_remain_ordinary_custom_names_in_scripts() {
    let source = "Suite { Return |1| }\nCase { Return |2| }\nLibrary { Return |3| }\nNamed { Return |4| }\nTags { Return |5| }\n|answer| = |@{ Suite } + @{ Case } + @{ Library } + @{ Named } + @{ Tags }|";
    let result = Engine::default().run_source("compatibility", source, RunOptions::default());
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.variables["answer"].to_string(), "15");
}

#[test]
fn declarations_accept_multiline_metadata_and_reject_executable_or_misordered_headers() {
    let suite = parse("# suite header\r\nsUiTe\r\n|\"s\"|\r\nnAmEd\r\n|\"Display\"|\r\ntAgS\r\n|[\r\n\"fast\",\r\n]|\r\n{\r\nlIbRaRy { Value { Return |7| } }\r\ncAsE\r\n|\"a\"|\r\n{ |answer| = Value }\r\n}\r\n");
    assert_eq!(suite.metadata().name(), "Display");
    let result = Engine::default().run_program(&suite.program(0).unwrap(), RunOptions::default());
    assert_eq!(result.variables["answer"].to_string(), "7");
    for source in [
        "Case |\"a\"| {}",
        "Suite |\"s\"| { Case {} }",
        "Suite |\"s\"| { Case |\"a\"| {} Library {} }",
        "Suite |\"s\"| { Library {} Library {} Case |\"a\"| {} }",
        "Suite |\"s\"| Named |@{ Value }| { Case |\"a\"| {} }",
        "Suite |\"s\"| Tags |[\"tag\"]| Named |\"Display\"| { Case |\"a\"| {} }",
        "Suite |\"s\"| { Case |\"a\"| {} } Suite |\"other\"| { Case |\"b\"| {} }",
    ] {
        assert!(
            Suite::parse("invalid", source).is_err(),
            "accepted {source}"
        );
    }
}

#[test]
fn metadata_boundaries_count_utf8_bytes_and_ids_allow_only_the_documented_alphabet() {
    for id in ["0", "A.b_c-9", &"x".repeat(MAX_ID_BYTES)] {
        assert!(valid_id(id), "{id}");
        assert!(valid_case_id(&format!("{id}/{id}")));
    }
    for id in [
        "",
        ".x",
        "-x",
        "_x",
        "a/b",
        "a b",
        "é",
        &"x".repeat(MAX_ID_BYTES + 1),
    ] {
        assert!(!valid_id(id), "{id}");
    }
    for id in ["s", "/c", "s/", "s/c/m", "s/é"] {
        assert!(!valid_case_id(id));
    }
    for (name, accepted) in [
        ("é".repeat(256), true),
        (format!("{}a", "é".repeat(256)), false),
    ] {
        assert_eq!(
            Suite::parse(
                "names",
                &format!("Suite |\"s\"| Named |\"{name}\"| {{ Case |\"a\"| {{}} }}")
            )
            .is_ok(),
            accepted
        );
    }
    for count in [MAX_TAGS, MAX_TAGS + 1] {
        let tags = (0..count)
            .map(|i| format!("\"t{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            Suite::parse(
                "tags",
                &format!("Suite |\"s\"| Tags |[{tags}]| {{ Case |\"a\"| {{}} }}")
            )
            .is_ok(),
            count == MAX_TAGS
        );
    }
    for (tag, accepted) in [
        ("é".repeat(64), true),
        (format!("{}a", "é".repeat(64)), false),
        (String::new(), false),
    ] {
        assert_eq!(
            Suite::parse(
                "tag",
                &format!("Suite |\"s\"| Tags |[\"{tag}\"]| {{ Case |\"a\"| {{}} }}")
            )
            .is_ok(),
            accepted
        );
    }
}

#[test]
fn selection_admits_exact_suite_case_and_source_limits_before_filtering() {
    let suites = (0..=MAX_SUITES)
        .map(|i| parse(&format!("Suite |\"s{i}\"| {{ Case |\"c\"| {{}} }}")))
        .collect::<Vec<_>>();
    assert_eq!(
        Selection::default()
            .select(&suites[..MAX_SUITES])
            .unwrap()
            .len(),
        MAX_SUITES
    );
    assert_eq!(
        rejected(Selection::default().select(&suites)).code(),
        DiagnosticCode::ResourceLimit
    );
    let declarations = (0..MAX_SUITE_CASES)
        .map(|i| format!("Case |\"c{i}\"| {{}}\n"))
        .collect::<String>();
    let mut suites = (0..4)
        .map(|i| parse(&format!("Suite |\"s{i}\"| {{ {declarations} }}")))
        .collect::<Vec<_>>();
    assert_eq!(
        Selection::default().select(&suites).unwrap().len(),
        MAX_SELECTED_CASES
    );
    suites.push(parse("Suite |\"over\"| { Case |\"c\"| {} }"));
    assert_eq!(
        rejected(
            Selection {
                cases: vec!["s0/c0".into()],
                ..Default::default()
            }
            .select(&suites)
        )
        .code(),
        DiagnosticCode::ResourceLimit
    );
    let mut suites = (0..8)
        .map(|i| {
            let mut source = format!("Suite |\"s{i}\"| {{ Case |\"c\"| {{}} }}\n#");
            source.extend(std::iter::repeat_n(
                'x',
                DEFAULT_SOURCE_BYTES - source.len(),
            ));
            parse(&source)
        })
        .collect::<Vec<_>>();
    assert_eq!(Selection::default().select(&suites).unwrap().len(), 8);
    suites.push(parse("Suite |\"over\"| { Case |\"c\"| {} }"));
    assert_eq!(
        rejected(Selection::default().select(&suites)).code(),
        DiagnosticCode::ResourceLimit
    );
}

#[test]
fn selector_bounds_are_checked_even_for_an_empty_rerun_and_errors_do_not_copy_huge_ids() {
    let suites = [parse("Suite |\"s\"| Tags |[\"é\"]| { Case |\"a\"| {} }")];
    let long = "é".repeat(MAX_ID_BYTES / 2);
    let tagged = [parse(&format!(
        "Suite |\"s\"| Tags |[\"{long}\"]| {{ Case |\"a\"| {{}} }}"
    ))];
    assert_eq!(
        Selection {
            tags: vec![long.clone()],
            ..Default::default()
        }
        .select(&tagged)
        .unwrap()
        .len(),
        1
    );
    assert_eq!(
        Selection {
            exclude_tags: vec![long],
            ..Default::default()
        }
        .select(&suites)
        .unwrap()
        .len(),
        1
    );
    let exact = Selection {
        cases: vec!["s/a".into(); MAX_SELECTED_CASES],
        tags: vec!["é".into(); MAX_TAGS],
        ..Default::default()
    };
    assert_eq!(exact.select(&suites).unwrap().len(), 1);
    for selection in [
        Selection {
            cases: vec!["s/a".into(); MAX_SELECTED_CASES + 1],
            ..Default::default()
        },
        Selection {
            failed: Some(vec!["s/a".into(); MAX_SELECTED_CASES + 1]),
            ..Default::default()
        },
        Selection {
            tags: vec!["é".into(); MAX_TAGS + 1],
            ..Default::default()
        },
        Selection {
            exclude_tags: vec!["x".into(); MAX_TAGS + 1],
            ..Default::default()
        },
        Selection {
            cases: vec!["x".repeat(100_000)],
            failed: Some(vec![]),
            ..Default::default()
        },
    ] {
        let error = rejected(selection.select(&suites));
        assert!(error.to_string().len() < 1024);
    }
    for tag in [String::new(), "\n".into(), "a".repeat(MAX_ID_BYTES + 1)] {
        for excluded in [true, false] {
            let mut selection = Selection {
                failed: Some(vec![]),
                ..Default::default()
            };
            if excluded {
                selection.exclude_tags.push(tag.clone());
            } else {
                selection.tags.push(tag.clone());
            }
            assert!(selection.select(&suites).is_err());
        }
    }
    assert!(Selection {
        tags: vec!["É".into()],
        ..Default::default()
    }
    .select(&suites)
    .is_err());
}
