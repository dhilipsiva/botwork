use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    grammar::{BWErr, BWParser, Literal, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    signature::StatementSignature,
};
use pest::Parser;
use std::collections::BTreeMap;

fn options(diagnostics: DiagnosticLimits) -> RunOptions {
    RunOptions {
        variables: BTreeMap::from([("seed".into(), Literal::Int(7))]),
        limits: RunLimits {
            diagnostics,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn syntax_messages_preserve_parser_output_at_exact_quotas_and_reject_before_any_effects() {
    for source in [
        "Log |1|\n|broken| = |1 +|",
        "# தமிழ்\r\n\t|🙂| = |1|\r\n",
        "Log |1\r\n",
        "If |false| { |x| = |[1,,2]| }",
        "Read |x| { Return |x +| }",
        "\n\n\n\n\n\n\n\n\n\n\t|x| = |1 +|",
    ] {
        let parser = BWParser::parse(Rule::botwork, source).unwrap_err();
        let baseline = Program::parse_detailed("syntax-é", source).unwrap_err();
        let BWErr::ParsingError(detail) = baseline.error.as_ref() else {
            panic!("syntax")
        };
        assert_eq!(detail, &parser.to_string());
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let exact = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        let run = Engine::default().run_source("syntax-é", source, options(exact.clone()));
        assert_eq!(run.outcome(), RunOutcome::Failed);
        assert_eq!(run.steps, 0);
        assert_eq!(run.variables["seed"].to_string(), "7");
        assert_eq!(
            run.result.unwrap_err().to_value().to_string(),
            baseline.to_value().to_string()
        );
        for dimension in 0..5 {
            let mut diagnostics = exact.clone();
            match dimension {
                0 => diagnostics.diagnostics = 0,
                1 => diagnostics.depth = 0,
                2 => diagnostics.text_bytes -= 1,
                3 => diagnostics.source_bytes -= 1,
                _ => diagnostics.text_bytes = 0,
            }
            let run = Engine::default().run_source("syntax-é", source, options(diagnostics));
            assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
            assert_eq!(run.steps, 0);
            assert_eq!(run.variables["seed"].to_string(), "7");
            let error = run.result.unwrap_err();
            assert_eq!(error.causes[0].code(), DiagnosticCode::Syntax);
            assert!(error.causes[0].span.is_none());
            let omissions = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omissions.detail_fields, 1);
            let source = omissions.source.as_ref().unwrap();
            assert_eq!(source.start_byte, baseline.span.as_ref().unwrap().start());
            assert_eq!(source.end_byte, baseline.span.as_ref().unwrap().end());
            let BWErr::ParsingError(detail) = error.causes[0].error.as_ref() else {
                panic!("syntax")
            };
            assert!(detail.contains("source excerpt omitted"));
            assert!(detail.len() <= 256);
        }
    }
}

#[test]
fn native_headers_keep_their_exact_syntax_details_and_standalone_defaults() {
    for header in ["", "Read |🙂|", "Read |x +|", "Read {}", "Read |"] {
        let parser = BWParser::parse(Rule::native_signature, header).unwrap_err();
        let error = StatementSignature::native(header).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Syntax);
        let BWErr::ParsingError(detail) = error.error.as_ref() else {
            panic!("syntax")
        };
        assert_eq!(detail, &parser.to_string());
        assert_eq!(error.span.as_ref().unwrap().source().name(), "<native>");
    }
}

#[test]
fn syntax_rejection_does_not_change_source_limit_or_stop_priority_and_fresh_runs_work() {
    let engine = Engine::default();
    let mut settings = options(DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    });
    settings.limits.source_bytes = 0;
    let run = engine.run_source("source", "|x| = |1 +|", settings);
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
    assert!(!error
        .causes
        .iter()
        .any(|cause| cause.code() == DiagnosticCode::Syntax));
    let settings = options(DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    });
    settings.control.cancel();
    assert_eq!(
        engine
            .run_source("stopped", "|x| = |1 +|", settings)
            .outcome(),
        RunOutcome::Cancelled
    );
    let run = engine.run_source(
        "fresh",
        "|answer| = |7|",
        options(DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(run.variables["answer"].to_string(), "7");
}
