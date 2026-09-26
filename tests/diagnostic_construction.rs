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

#[test]
fn formatted_signature_errors_preserve_exact_metadata_and_rejected_returns_keep_completed_effects()
{
    use botwork::core::{
        grammar::Literal,
        signature::{StatementSignature, ValueKind},
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    for returning in [false, true] {
        let entries = Arc::new(AtomicUsize::new(0));
        let entered = entries.clone();
        let signature = StatementSignature::native("Read |value|").unwrap();
        let signature = if returning {
            signature.returns(ValueKind::String)
        } else {
            signature.parameter("value", ValueKind::Int).unwrap()
        };
        let mut engine = Engine::default();
        engine
            .register_native_with_signature(signature, move |_, _| {
                entered.fetch_add(1, Ordering::SeqCst);
                Ok(Literal::Bool(true))
            })
            .unwrap();
        let source = "Outer { Read |true| }\nOuter";
        let baseline = engine
            .run_source("signature", source, RunOptions::default())
            .result
            .unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::IncompatibleType);
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let exact = DiagnosticLimits {
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
            call_frames: size.call_frames,
            ..DiagnosticLimits::default()
        };
        let accepted = engine
            .run_source("signature", source, options(exact.clone()))
            .result
            .unwrap_err();
        assert_eq!(
            accepted.to_value().to_string(),
            baseline.to_value().to_string()
        );
        let rejected = engine
            .run_source(
                "signature",
                source,
                options(DiagnosticLimits {
                    text_bytes: size.text_bytes - 1,
                    ..exact
                }),
            )
            .result
            .unwrap_err();
        assert_eq!(rejected.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(rejected.causes[0].code(), DiagnosticCode::IncompatibleType);
        assert_eq!(
            rejected.causes[0].omissions.as_ref().unwrap().call_frames,
            size.call_frames
        );
        let run = engine.run_source(
            "handler",
            "|before| = |1|\nTry { Read |true| } Catch { |caught| = |true| }\n|after| = |2|",
            options(DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            }),
        );
        assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
        assert_eq!(run.variables["before"].to_string(), "1");
        assert!(!run.variables.contains_key("caught") && !run.variables.contains_key("after"));
        assert_eq!(
            entries.load(Ordering::SeqCst),
            if returning { 4 } else { 0 }
        );
    }
}

#[test]
fn rejected_argument_message_keeps_required_effects_and_skips_later_arguments_and_entry() {
    use botwork::core::{
        grammar::Literal,
        signature::{StatementSignature, ValueKind},
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let effects = Arc::new(AtomicUsize::new(0));
    let touched = effects.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Touch", move |_, _| {
            touched.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Bool(true))
        })
        .unwrap();
    engine
        .register_native("Later", |_, _| panic!("later argument entered"))
        .unwrap();
    engine
        .register_native_with_signature(
            StatementSignature::native("Read |first| Then |second|")
                .unwrap()
                .parameter("first", ValueKind::Int)
                .unwrap(),
            |_, _| panic!("callee entered"),
        )
        .unwrap();
    let run = engine.run_source(
        "arguments",
        "Read |@{ Touch }| Then |@{ Later }|",
        options(DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[test]
fn access_failures_admit_all_fields_at_exact_limits_with_unchanged_paths_and_locations() {
    use botwork::core::grammar::BWErr;
    for (value, suffix, reason) in [
        ("{}", ".é", "map key does not exist"),
        ("{}", "[1]", "map key must be a string"),
        (
            "[]",
            ".bad",
            "array index must contain ASCII decimal digits",
        ),
        ("[]", "[true]", "array index must be a nonnegative integer"),
        ("[7]", "[12]", "array index is out of bounds for length 1"),
        (
            "[]",
            ".999999999999999999999999999999999",
            "array index is out of bounds for length 0",
        ),
        ("true", ".key", "value is neither a map nor an array"),
    ] {
        let source = format!("|data| = |{value}|\nOuter {{ |out| = |data{suffix}| }}\nOuter");
        let engine = Engine::default();
        let baseline = engine
            .run_source("access", &source, RunOptions::default())
            .result
            .unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::CollectionAccess);
        let BWErr::CollectionAccessError {
            path,
            segment,
            reason: actual,
        } = baseline.error.as_ref()
        else {
            panic!("category")
        };
        assert_eq!(path, &format!("data{suffix}"));
        assert_eq!(segment, suffix.strip_prefix('.').unwrap_or(suffix));
        assert_eq!(actual, reason);
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        for fits in [false, true] {
            let error = engine
                .run_source(
                    "access",
                    &source,
                    options(DiagnosticLimits {
                        text_bytes: size.text_bytes - usize::from(!fits),
                        source_bytes: size.source_bytes,
                        call_frames: size.call_frames,
                        ..DiagnosticLimits::default()
                    }),
                )
                .result
                .unwrap_err();
            if fits {
                assert_eq!(
                    error.to_value().to_string(),
                    baseline.to_value().to_string()
                );
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::CollectionAccess);
                assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
                assert_eq!(
                    error.causes[0]
                        .omissions
                        .as_ref()
                        .unwrap()
                        .source
                        .as_ref()
                        .unwrap()
                        .start_byte,
                    baseline.span.as_ref().unwrap().start()
                );
            }
        }
    }
}

#[test]
fn access_construction_failure_preserves_computed_key_effects_and_skips_later_keys() {
    use botwork::core::grammar::Literal;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let effects = Arc::new(AtomicUsize::new(0));
    let calls = effects.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Key", move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::String("missing".into()))
        })
        .unwrap();
    engine
        .register_native("Later", |_, _| panic!("later key entered"))
        .unwrap();
    let source = "|before| = |1|\n|data| = |{}|\nTry { |out| = |data[@{ Key }][@{ Later }]| } Catch { |caught| = |true| }\n|after| = |2|";
    let run = engine.run_source(
        "effects",
        source,
        options(DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.variables["before"].to_string(), "1");
    assert!(
        !run.variables.contains_key("caught")
            && !run.variables.contains_key("out")
            && !run.variables.contains_key("after")
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::CollectionAccess);
    assert_eq!(
        error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .start_byte,
        source.find("[@{ Key }]").unwrap()
    );
}

#[test]
fn admitted_access_details_remain_available_to_catch_and_restore_the_previous_binding() {
    let source = "|error| = |7|\n|data| = |{}|\nTry { |out| = |data.é| } Catch |error| { |path| = |error.details.path|\n|segment| = |error.details.segment|\n|reason| = |error.details.reason| }";
    let run = Engine::default().run_source(
        "caught",
        source,
        options(DiagnosticLimits {
            text_bytes: "expression".len()
                + "data.é".len()
                + "é".len()
                + "map key does not exist".len(),
            ..DiagnosticLimits::default()
        }),
    );
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert_eq!(run.variables["error"].to_string(), "7");
    assert_eq!(run.variables["path"].to_string(), "data.é");
    assert_eq!(run.variables["segment"].to_string(), "é");
    assert_eq!(
        run.variables["reason"].to_string(),
        "map key does not exist"
    );
}
