use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    syntax_limits::SyntaxLimits,
};
use std::collections::BTreeMap;

#[test]
fn source_and_syntax_guards_preserve_exact_prefixes_or_bounded_original_limit_evidence() {
    for (source, bytes, syntax) in [
        ("🙂", 2, SyntaxLimits::default()),
        ("Log |1|\nBreak", 8, SyntaxLimits::default()),
        (
            "Log |[[1]]|",
            100,
            SyntaxLimits {
                nesting: 2,
                operators: 64,
            },
        ),
        (
            "Log |1 + 2|",
            100,
            SyntaxLimits {
                nesting: 32,
                operators: 0,
            },
        ),
    ] {
        let baseline = Program::parse_bounded("guard-é", source, bytes, &syntax).unwrap_err();
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let exact = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        for dimension in 0..5 {
            let mut diagnostics = exact.clone();
            match dimension {
                0 => {}
                1 => diagnostics.source_bytes -= 1,
                2 => diagnostics.text_bytes -= 1,
                3 => diagnostics.diagnostics = 0,
                _ => diagnostics.depth = 0,
            }
            let run = Engine::default().run_source(
                "guard-é",
                source,
                RunOptions {
                    variables: BTreeMap::from([("seed".into(), Literal::Int(7))]),
                    limits: RunLimits {
                        source_bytes: bytes,
                        syntax: syntax.clone(),
                        diagnostics,
                        ..RunLimits::default()
                    },
                    ..RunOptions::default()
                },
            );
            assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
            assert_eq!(run.steps, 0);
            assert_eq!(run.variables["seed"].to_string(), "7");
            let error = run.result.unwrap_err();
            if source.len() > bytes {
                // Engine's earlier size check deliberately has no source owner.
                // Keep its ordering and representation; Program guards retain prefixes.
                assert!(error.span.is_none());
                if dimension <= 1 {
                    assert_eq!(error.error.to_string(), baseline.error.to_string());
                    assert!(error.causes.is_empty());
                } else {
                    assert_eq!(
                        error.causes[0].error.to_string(),
                        baseline.error.to_string()
                    );
                    assert!(error.causes[0].omissions.as_ref().unwrap().source.is_none());
                }
                continue;
            }
            if dimension == 0 {
                assert_eq!(
                    error.to_value().to_string(),
                    baseline.to_value().to_string()
                );
                let span = error.span.as_ref().unwrap();
                assert_eq!(span.source().text(), &source[..span.end()]);
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(
                    error.causes[0].error.to_string(),
                    baseline.error.to_string()
                );
                assert!(error.causes[0].span.is_none());
                let omitted = error.causes[0].omissions.as_ref().unwrap();
                assert_eq!(omitted.detail_fields, 0);
                let evidence = omitted.source.as_ref().unwrap();
                assert_eq!(evidence.file, "guard-é");
                let span = baseline.span.as_ref().unwrap();
                assert_eq!(
                    (evidence.start_byte, evidence.end_byte),
                    (span.start(), span.end())
                );
            }
        }
    }
}

#[test]
fn standalone_guard_admission_uses_default_source_quota_and_preserves_invalid_configuration() {
    let source = "🙂";
    let name = "n".repeat(DiagnosticLimits::default().source_bytes - source.len());
    let error = Program::parse_bounded(&name, source, 0, &SyntaxLimits::default()).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(
        DiagnosticLimits::default()
            .check(&error)
            .unwrap()
            .source_bytes,
        DiagnosticLimits::default().source_bytes
    );
    assert!(error.causes.is_empty());
    let error = Program::parse_bounded(&format!("{name}é"), source, 0, &SyntaxLimits::default())
        .unwrap_err();
    assert!(
        error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .file_truncated
    );
    assert!(error.causes[0].span.is_none());
    assert!(error.causes[0].error.to_string().contains("source bytes"));

    let name = "n".repeat(DiagnosticLimits::default().source_bytes + 1);
    let error = Program::parse_bounded(
        &name,
        "",
        0,
        &SyntaxLimits {
            nesting: usize::MAX,
            operators: 0,
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::RunConfiguration);
    assert!(error.causes[0]
        .error
        .to_string()
        .contains("Syntax limits cannot exceed"));
}

#[test]
fn owned_program_syntax_guards_reject_before_effects_without_retaining_original_sources() {
    let program = Program::parse("owned", "|before| = |7|\nLog |[[1]]|").unwrap();
    let source = std::sync::Arc::downgrade(&program.source);
    let run = Engine::default().run_program(
        &program,
        RunOptions {
            limits: RunLimits {
                syntax: SyntaxLimits {
                    nesting: 2,
                    operators: 64,
                },
                diagnostics: DiagnosticLimits {
                    source_bytes: 0,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 0);
    assert!(run.variables.is_empty());
    let error = run.result.unwrap_err();
    assert!(error.causes[0].error.to_string().contains("syntax nesting"));
    assert!(error.causes[0].span.is_none());
    drop(program);
    assert!(source.upgrade().is_none());
}

#[test]
fn stopped_guard_runs_keep_cancellation_priority_and_leave_fresh_runs_usable() {
    let settings = || RunOptions {
        limits: RunLimits {
            source_bytes: 0,
            diagnostics: DiagnosticLimits {
                source_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let engine = Engine::default();
    let stopped = settings();
    stopped.control.cancel();
    assert_eq!(
        engine.run_source("stopped", "🙂", stopped).outcome(),
        RunOutcome::Cancelled
    );
    assert_eq!(
        engine.run_source("fresh", "", settings()).outcome(),
        RunOutcome::Succeeded
    );
}
