use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticLimits},
    eval::{botwork_detailed, evaluate_program_detailed, Context},
    grammar::{BWParser, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
};
use pest::Parser;

fn limits(diagnostics: DiagnosticLimits) -> RunLimits {
    RunLimits {
        diagnostics,
        ..RunLimits::default()
    }
}
fn options(diagnostics: DiagnosticLimits) -> RunOptions {
    RunOptions {
        limits: limits(diagnostics),
        ..RunOptions::default()
    }
}

#[test]
fn exact_message_context_limits_preserve_full_errors_and_one_less_rejects_before_catch() {
    for source in [
        "Outer { |x| = |absent| }\nOuter",
        "Outer { |x| = |absent.key| }\nOuter",
        "Outer { Missing }\nOuter",
        "Outer { Panic }\nOuter",
    ] {
        let mut engine = Engine::default();
        engine
            .register_native("Panic", |_, _| panic!("callback"))
            .unwrap();
        let baseline = engine
            .run_source("source", source, RunOptions::default())
            .result
            .unwrap_err();
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let exact = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        let error = engine
            .run_source("source", source, options(exact.clone()))
            .result
            .unwrap_err();
        assert_eq!(
            error.to_value().to_string(),
            baseline.to_value().to_string()
        );
        let error = engine
            .run_source(
                "source",
                source,
                options(DiagnosticLimits {
                    text_bytes: size.text_bytes - 1,
                    ..exact
                }),
            )
            .result
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), baseline.code());
        let omitted = error.causes[0].omissions.as_ref().unwrap();
        assert_eq!(omitted.call_frames, size.call_frames);
        assert!(omitted.source.is_some());
    }
}

#[test]
fn construction_rejection_preserves_prior_effects_and_handler_binding_cleanup() {
    let source = "|error| = |7|\nTry { First } Catch |error| { |before| = |1|\nThis missing statement has a long name }\n|after| = |2|";
    let run = Engine::default().run_source(
        "handler",
        source,
        options(DiagnosticLimits {
            text_bytes: 32,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.variables["error"].to_string(), "7");
    assert_eq!(run.variables["before"].to_string(), "1");
    assert!(!run.variables.contains_key("after"));
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedStatement);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
}

#[test]
fn undefined_name_rejection_latches_only_the_requesting_context_across_pair_entry() {
    let configuration = limits(DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    });
    let mut context = Context::with_limits(configuration).unwrap();
    let mut sibling = context.clone();
    let pair = BWParser::parse(Rule::botwork, "|x| = |absent|")
        .unwrap()
        .next()
        .unwrap();
    let error = botwork_detailed(pair, &mut context).unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::UndefinedVariable);
    assert!(error.causes[0].omissions.as_ref().unwrap().source.is_some());
    let empty = Program::parse("empty", "").unwrap();
    assert!(evaluate_program_detailed(&empty, &mut context).is_err());
    evaluate_program_detailed(&empty, &mut sibling).unwrap();
}

#[test]
fn observed_native_panic_cancellation_keeps_priority_over_construction_limits() {
    let mut engine = Engine::default();
    engine
        .register_native("Stop", |_, environment| {
            environment.control().cancel();
            panic!("callback after stop")
        })
        .unwrap();
    let run = engine.run_source(
        "stop",
        "Try { Stop } Catch { |unexpected| = |true| }",
        options(DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Cancelled);
    assert!(!run.variables.contains_key("unexpected"));
    let error = run.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::Cancelled);
    assert!(error.omissions.is_some());
    assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
}

#[test]
fn admitted_unicode_name_errors_remain_catchable_with_unchanged_source_coordinates() {
    let source = "Try { |x| = |absént| } Catch |error| { |code| = |error.code|\n|name| = |error.details.name|\n|start| = |error.source.start_byte| }";
    let run = Engine::default().run_source(
        "unicode",
        source,
        options(DiagnosticLimits {
            text_bytes: "expression".len() + "absént".len(),
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(run.variables["code"].to_string(), "BW2001");
    assert_eq!(run.variables["name"].to_string(), "absént");
    assert_eq!(
        run.variables["start"].to_string(),
        source.find("absént").unwrap().to_string()
    );
}
