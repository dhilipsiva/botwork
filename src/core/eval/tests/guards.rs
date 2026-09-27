use super::super::*;
use crate::core::diagnostic::{DiagnosticCode, DiagnosticLimits};

fn context(text_bytes: usize) -> Context {
    Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap()
}

#[test]
fn native_numeric_guards_admit_argument_or_return_context_before_message_construction() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for invalid_return in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let install = |context: &mut Context| {
            let calls = calls.clone();
            context
                .register_native("Read |value|", move |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(Literal::Array(vec![Literal::Float(f32::NAN)]))
                })
                .unwrap();
            // Private insertion exercises a defensive invalid argument unreachable from DSL literals.
            context
                .set_variable(
                    "value",
                    if invalid_return {
                        Literal::Int(1)
                    } else {
                        Literal::Array(vec![Literal::Float(f32::INFINITY)])
                    },
                )
                .unwrap();
        };
        let program = Program::parse("caller-é", "Read |value|").unwrap();
        let mut baseline_context = Context::default();
        install(&mut baseline_context);
        let baseline = evaluate_program_detailed(&program, &mut baseline_context).unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::Arithmetic);
        assert_eq!(baseline.call_stack.len(), usize::from(invalid_return));
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        for text_bytes in [size.text_bytes, size.text_bytes - 1] {
            calls.store(0, Ordering::SeqCst);
            let mut context = context(text_bytes);
            install(&mut context);
            let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(invalid_return));
            if text_bytes == size.text_bytes {
                assert_eq!(error.to_string(), baseline.to_string());
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
                assert!(context.checkpoint().is_err());
            }
            assert!(context.calls.is_empty());
        }
    }
}

#[test]
fn conditional_guards_admit_entered_calls_and_preserve_effects_and_cleanup() {
    for statement in ["If |1| {}", "While |1| {}", "For |item| In |1| {}"] {
        let program = Program::parse(
            "guards-é",
            &format!("Read {{ {statement} }}\n|before| = |1|\nRead\n|after| = |2|"),
        )
        .unwrap();
        let baseline = evaluate_program_detailed(&program, &mut Context::default()).unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::IncompatibleType);
        assert_eq!(baseline.span.as_ref().unwrap().text(), "1");
        assert_eq!(baseline.call_stack.len(), 1);
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        for text_bytes in [size.text_bytes, size.text_bytes - 1] {
            let mut context = context(text_bytes);
            let mut sibling = context.clone();
            let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
            if text_bytes == size.text_bytes {
                assert_eq!(error.to_string(), baseline.to_string());
                context.checkpoint().unwrap();
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), baseline.code());
                assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
                assert!(context.checkpoint().is_err());
            }
            assert_eq!(context.frames[0].variables["before"].value.to_string(), "1");
            assert!(!context.frames[0].variables.contains_key("after"));
            assert!(context.calls.is_empty() && context.handlers.is_empty());
            assert_eq!(context.frames.len(), 1);
            evaluate_program_detailed(&Program::parse("valid", "|x| = |7|").unwrap(), &mut sibling)
                .unwrap();
        }
    }
}

#[test]
fn defensive_completion_guards_admit_statement_sources_before_retention() {
    use crate::core::grammar::BWParser;
    use pest::Parser;

    for source in ["Return |7|", "Break", "Continue", "Rethrow"] {
        for reject in [false, true] {
            let pair = BWParser::parse(Rule::botwork, source)
                .unwrap()
                .next()
                .unwrap();
            let Node::Statement(statement) = ast::from_pair(pair).unwrap() else {
                panic!("statement")
            };
            let owner = Arc::downgrade(statement.span.source());
            let mut context = context(if reject { 0 } else { usize::MAX });
            let error = evaluate_statement(&statement, &mut context)
                .and_then(|completion| finish_script(completion, &context, &statement.span))
                .unwrap_err();
            if reject {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::InvalidControl);
                assert!(context.checkpoint().is_err());
            } else {
                assert_eq!(error.code(), DiagnosticCode::InvalidControl);
                assert_eq!(error.span.as_ref().unwrap().text(), source);
                context.checkpoint().unwrap();
            }
            assert!(context.calls.is_empty() && context.handlers.is_empty());
            drop(statement);
            assert_eq!(owner.upgrade().is_none(), reject);
            drop(error);
            assert!(owner.upgrade().is_none());
        }
    }
}

#[test]
fn malformed_call_guard_rejects_before_argument_evaluation_or_native_entry() {
    let program = Program::parse("caller", "Read |missing|").unwrap();
    let StatementKind::Invoke(mut call) = program.statements[0].kind.clone() else {
        panic!("call")
    };
    call.arguments.push(call.arguments[0].clone());
    let mut baseline_context = Context::default();
    baseline_context
        .register_native("Read |value|", |_| panic!("native entry"))
        .unwrap();
    let baseline = invoke(&call, &mut baseline_context).unwrap_err();
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for text_bytes in [size.text_bytes, size.text_bytes - 1, 0] {
        let mut context = context(text_bytes);
        context
            .register_native("Read |value|", |_| panic!("native entry"))
            .unwrap();
        let error = invoke(&call, &mut context).unwrap_err();
        if text_bytes == size.text_bytes {
            assert_eq!(error.to_string(), baseline.to_string());
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::ParameterCount);
        }
        assert!(context.expression_visits.borrow().is_empty());
        assert!(context.calls.is_empty());
    }
}

#[test]
fn configured_native_guard_admits_the_entered_call_before_missing_environment_error() {
    let program = Program::parse("caller", "Read").unwrap();
    let install = |context: &mut Context| {
        context
            .register_run_native(StatementSignature::native("Read").unwrap(), |_, _| {
                panic!("native entry")
            })
            .unwrap()
    };
    let mut baseline_context = Context::default();
    install(&mut baseline_context);
    let baseline = evaluate_program_detailed(&program, &mut baseline_context).unwrap_err();
    assert_eq!(baseline.code(), DiagnosticCode::RunConfiguration);
    assert_eq!(baseline.call_stack.len(), 1);
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for text_bytes in [size.text_bytes, size.text_bytes - 1] {
        let mut context = context(text_bytes);
        install(&mut context);
        let error = evaluate_program_detailed(&program, &mut context).unwrap_err();
        if text_bytes == size.text_bytes {
            assert_eq!(error.to_string(), baseline.to_string());
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::RunConfiguration);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
        }
        assert!(context.calls.is_empty());
    }
}

#[test]
fn native_registration_guard_admits_header_source_and_latches_only_the_requester() {
    for configured in [false, true] {
        for source_bytes in [0, usize::MAX] {
            let program = Program::parse("definition-é", "Read {}").unwrap();
            let StatementKind::Define(definition) = &program.statements[0].kind else {
                panic!("definition")
            };
            let signature = definition.signature_metadata();
            let owner = Arc::downgrade(&program.source);
            drop(program);
            let mut context = Context::with_limits(RunLimits {
                diagnostics: DiagnosticLimits {
                    source_bytes,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            })
            .unwrap();
            let sibling = context.clone();
            let error = if configured {
                context.register_run_native(signature, |_, _| panic!("native entry"))
            } else {
                context.register_native_with_signature(signature, |_| panic!("native entry"))
            }
            .unwrap_err();
            assert!(context.frames[0].statements.is_empty());
            if source_bytes == 0 {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::Signature);
                assert!(owner.upgrade().is_none());
                assert!(context.checkpoint().is_err());
            } else {
                assert_eq!(error.code(), DiagnosticCode::Signature);
                assert_eq!(error.span.as_ref().unwrap().text(), "Read ");
                assert!(owner.upgrade().is_some());
                context.checkpoint().unwrap();
            }
            sibling.checkpoint().unwrap();
            drop(error);
            assert!(owner.upgrade().is_none());
        }
    }
}
