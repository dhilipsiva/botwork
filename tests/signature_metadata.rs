use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use botwork::core::{
    ast::{Program, StatementKind},
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
    signature::{StatementOrigin, StatementSignature, ValueKind, ValueKinds},
};

fn run(source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
    evaluate_program_detailed(
        &Program::parse_detailed("signatures.botwork", source)?,
        context,
    )
}

fn typed(header: &str, parameter: ValueKind, returns: ValueKind) -> StatementSignature {
    StatementSignature::native(header)
        .unwrap()
        .parameter("value", parameter)
        .unwrap()
        .returns(returns)
}

#[test]
fn builder_errors_preserve_exact_messages_locations_and_other_metadata_owners() {
    use botwork::core::{diagnostic::DiagnosticLimits, grammar::BWErr, run::RunLimits};
    let signature = StatementSignature::native("Read |value|").unwrap();
    for name in ["Value", "missing", "é\n`value`"] {
        let error = signature
            .clone()
            .parameter(name, ValueKind::Int)
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Signature);
        let BWErr::SignatureError(message) = error.error.as_ref() else {
            panic!("signature")
        };
        assert_eq!(
            message,
            &format!("Unknown parameter `{name}` in `Read |value|`")
        );
        let span = error.span.as_ref().unwrap();
        assert_eq!(span.text(), "Read |value|");
        assert_eq!(span.source().name(), "<native>");
        assert_eq!(span.line_column(), (1, 1));
        assert!(error.causes.is_empty() && error.omissions.is_none());
    }
    for error in [
        signature
            .clone()
            .documents_error(DiagnosticCode::Native, "\n \t")
            .unwrap_err(),
        signature
            .clone()
            .documents_error(DiagnosticCode::Native, "first")
            .unwrap()
            .documents_error(DiagnosticCode::Native, "second")
            .unwrap_err(),
    ] {
        assert_eq!(error.code(), DiagnosticCode::Signature);
        let BWErr::SignatureError(message) = error.error.as_ref() else {
            panic!("signature")
        };
        assert_eq!(
            message,
            "Document each error code once with a nonempty description"
        );
        assert!(error.omissions.is_none());
        assert_eq!(error.span.as_ref().unwrap().text(), "Read |value|");
    }
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native_with_signature(
            signature.parameter("value", ValueKind::Int).unwrap(),
            |values| Ok(values[0].clone()),
        )
        .unwrap();
    assert_eq!(run("Read |7|", &mut context).unwrap().to_string(), "7");
}

#[test]
fn dsl_derived_builder_errors_reject_oversized_sources_and_release_consumed_metadata() {
    use botwork::core::{
        ast_limits::AstLimits, diagnostic::DiagnosticLimits, syntax_limits::SyntaxLimits,
    };
    let filename = "é".repeat(DiagnosticLimits::default().source_bytes / 2);
    for documentation_error in [false, true] {
        let program = Program::parse_with_budgets(
            &filename,
            "Read |value| { Return |value| }",
            1024,
            &SyntaxLimits::default(),
            &AstLimits {
                source_bytes: usize::MAX,
                ..AstLimits::default()
            },
        )
        .unwrap();
        let owner = Arc::downgrade(&program.source);
        let StatementKind::Define(definition) = program.statements[0].kind() else {
            panic!("definition")
        };
        let signature = definition.signature_metadata();
        let start = signature.header().start();
        let end = signature.header().end();
        drop(program);
        let error = if documentation_error {
            signature.documents_error(DiagnosticCode::Native, " ")
        } else {
            signature.parameter("missing", ValueKind::Int)
        }
        .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert!(owner.upgrade().is_none());
        let cause = &error.causes[0];
        assert_eq!(cause.code(), DiagnosticCode::Signature);
        assert!(cause.span.is_none());
        let omitted = cause.omissions.as_ref().unwrap();
        assert_eq!(omitted.detail_fields, 0);
        let evidence = omitted.source.as_ref().unwrap();
        assert!(evidence.file_truncated && evidence.file.len() <= 256);
        assert_eq!((evidence.start_byte, evidence.end_byte), (start, end));
    }
}

#[test]
fn all_kinds_and_every_nonempty_union_have_deterministic_membership_and_display() {
    let values = [
        Literal::None,
        Literal::Int(0),
        Literal::Float(0.0),
        Literal::Bool(false),
        Literal::String(String::new()),
        Literal::Array(vec![]),
        Literal::Map(Default::default()),
    ];
    for (kind, value) in ValueKind::ALL.into_iter().zip(values) {
        assert_eq!(value.kind(), kind);
        assert_eq!(ValueKinds::one(kind).to_string(), kind.as_str());
    }
    for mask in 1..128_u8 {
        let expected = ValueKind::ALL
            .into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, kind)| kind)
            .collect::<Vec<_>>();
        let kinds = expected
            .iter()
            .copied()
            .map(ValueKinds::one)
            .reduce(ValueKinds::union)
            .unwrap();
        assert_eq!(kinds.iter().collect::<Vec<_>>(), expected);
        for (index, kind) in ValueKind::ALL.into_iter().enumerate() {
            assert_eq!(kinds.contains(kind), mask & (1 << index) != 0);
        }
        if mask == 127 {
            assert_eq!(kinds, ValueKinds::ANY);
            assert_eq!(kinds.to_string(), "Any");
        } else {
            assert_eq!(
                kinds.to_string(),
                expected
                    .iter()
                    .map(|kind| kind.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
        }
    }
}

#[test]
fn native_metadata_retains_header_names_kinds_origin_and_ordered_error_documentation() {
    let metadata = StatementSignature::native("Read |path| with |Mode|")
        .unwrap()
        .parameter("path", ValueKind::String)
        .unwrap()
        .parameter(
            "Mode",
            ValueKinds::one(ValueKind::Int).union(ValueKind::Bool.into()),
        )
        .unwrap()
        .returns(ValueKind::Array)
        .description("Read a record.")
        .documents_error(DiagnosticCode::Native, "Read failed.")
        .unwrap()
        .documents_error(DiagnosticCode::CollectionAccess, "Column missing.")
        .unwrap();
    assert_eq!(metadata.normalized(), "read|param|with|param|");
    assert_eq!(metadata.origin(), StatementOrigin::Native);
    assert_eq!(metadata.parameters()[0].span.text(), "path");
    assert_eq!(metadata.return_kinds(), ValueKind::Array.into());
    assert_eq!(metadata.documentation(), "Read a record.");
    assert_eq!(
        metadata
            .documented_errors()
            .iter()
            .map(|error| error.code)
            .collect::<Vec<_>>(),
        [DiagnosticCode::CollectionAccess, DiagnosticCode::Native]
    );
    assert_eq!(metadata.help(), "Read |path| with |Mode|\nRead a record.\n  path: String\n  Mode: Int | Bool\n  returns: Array\n  BW3004: Column missing.\n  BW4002: Read failed.");
}

#[test]
fn invalid_metadata_cannot_be_registered_and_does_not_replace_existing_contracts() {
    let metadata = StatementSignature::native("Value |value|").unwrap();
    for invalid in [
        metadata.clone().parameter("Value", ValueKind::Int),
        metadata
            .clone()
            .documents_error(DiagnosticCode::Native, " "),
        metadata
            .clone()
            .documents_error(DiagnosticCode::Native, "first")
            .unwrap()
            .documents_error(DiagnosticCode::Native, "second"),
    ] {
        let error = invalid.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::Signature);
        assert_eq!(error.span.unwrap().source().name(), "<native>");
    }
    let mut context = Context::default();
    context
        .register_native_with_signature(metadata, |_| Ok(Literal::Int(1)))
        .unwrap();
    assert_eq!(
        context
            .register_native_with_signature(
                typed("VALUE |value|", ValueKind::Int, ValueKind::Int),
                |_| Ok(Literal::Int(2))
            )
            .unwrap_err()
            .code(),
        DiagnosticCode::DuplicateStatement
    );
    assert_eq!(
        context
            .statement_signature("Value |x|")
            .unwrap()
            .unwrap()
            .parameters()[0]
            .accepted,
        ValueKinds::ANY
    );
    assert!(matches!(
        run("Value |true|", &mut context),
        Ok(Literal::Int(1))
    ));
}

#[test]
fn dsl_metadata_is_shared_dynamic_and_available_without_execution() {
    let program =
        Program::parse("definitions.botwork", "Mix |x| with |X| { Return |x + X| }").unwrap();
    let StatementKind::Define(definition) = program.statements[0].kind() else {
        panic!("definition")
    };
    let metadata = definition.signature_metadata();
    assert_eq!(metadata.origin(), StatementOrigin::Dsl);
    assert_eq!(metadata.header().text().trim(), "Mix |x| with |X|");
    assert_eq!(
        metadata
            .parameters()
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
        ["x", "X"]
    );
    assert!(metadata
        .parameters()
        .iter()
        .all(|parameter| parameter.accepted == ValueKinds::ANY));
    assert_eq!(metadata.return_kinds(), ValueKinds::ANY);
    assert!(metadata.documented_errors().is_empty());
    assert!(metadata.help().contains("not inferred"));
    let mut context = Context::default();
    assert!(context.statement_signatures().is_empty());
    assert_eq!(
        context
            .register_native_with_signature(metadata, |_| Ok(Literal::None))
            .unwrap_err()
            .code(),
        DiagnosticCode::Signature
    );
    evaluate_program_detailed(&program, &mut context).unwrap();
    let registered = context
        .statement_signature("m i x |a| with |b|")
        .unwrap()
        .unwrap();
    assert_eq!(registered.help(), definition.signature_metadata().help());
}

#[test]
fn argument_contracts_reject_each_wrong_kind_before_effects_and_later_arguments() {
    let mut context = Context::default();
    let entered = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&entered);
    context
        .register_native_with_signature(
            StatementSignature::native("Check |value| then |later|")
                .unwrap()
                .parameter("value", ValueKind::String)
                .unwrap(),
            move |_| {
                called.fetch_add(1, Ordering::SeqCst);
                Ok(Literal::None)
            },
        )
        .unwrap();
    for value in ["0", "0.0", "false", "[]", "{}"] {
        let source = format!("Outer {{ Check |{value}| then |missing| }}\nOuter");
        let mut context = context.clone();
        let error = run(&source, &mut context).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::IncompatibleType);
        assert_eq!(error.span.as_ref().unwrap().text(), value);
        assert!(error.to_string().contains("Parameter `value` (argument 1)"));
        assert_eq!(error.call_stack.len(), 1);
        assert_eq!(error.call_stack[0].signature, "outer");
    }
    context
        .register_native("None", |_| Ok(Literal::None))
        .unwrap();
    assert_eq!(
        run("|none| = None\nCheck |none| then |1|", &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::IncompatibleType
    );
    assert_eq!(entered.load(Ordering::SeqCst), 0);
    run("Check |\"yes\"| then |1|", &mut context).unwrap();
    assert_eq!(entered.load(Ordering::SeqCst), 1);
}

#[test]
fn kind_unions_do_not_coerce_values_or_weaken_nested_finite_validation() {
    let mut context = Context::default();
    let signature = StatementSignature::native("Number |value|")
        .unwrap()
        .parameter(
            "value",
            ValueKinds::one(ValueKind::Int).union(ValueKind::Float.into()),
        )
        .unwrap();
    context
        .register_native_with_signature(signature, |arguments| Ok(arguments[0].clone()))
        .unwrap();
    assert!(matches!(
        run("Number |1|", &mut context),
        Ok(Literal::Int(1))
    ));
    assert!(matches!(
        run("Number |1.0|", &mut context),
        Ok(Literal::Float(1.0))
    ));
    assert_eq!(
        run("Number |\"1\"|", &mut context).unwrap_err().code(),
        DiagnosticCode::IncompatibleType
    );
    context
        .register_native_with_signature(
            StatementSignature::native("Invalid")
                .unwrap()
                .returns(ValueKind::Array),
            |_| Ok(Literal::Array(vec![Literal::Float(f32::NAN)])),
        )
        .unwrap();
    assert_eq!(
        run("Invalid", &mut context).unwrap_err().code(),
        DiagnosticCode::Arithmetic
    );
}

#[test]
fn return_contract_failures_preserve_assignments_and_keep_the_entered_native_frame() {
    let mut context = Context::default();
    context
        .register_native_with_signature(
            typed("Invalid |value|", ValueKind::Int, ValueKind::String),
            |arguments| Ok(arguments[0].clone()),
        )
        .unwrap();
    let error = run("|answer| = |7|\n|answer| = Invalid |1|", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::IncompatibleType);
    assert!(error
        .to_string()
        .contains("Return value of `invalid|param|` requires String; got Int"));
    assert_eq!(error.call_stack[0].signature, "invalid|param|");
    assert!(matches!(
        run("|result| = |answer|", &mut context),
        Ok(Literal::Int(7))
    ));
    run(
        "Try { Invalid |1| } Catch |error| { |seen| = |error.code| }",
        &mut context,
    )
    .unwrap();
    assert_eq!(
        run("|result| = |seen|", &mut context).unwrap().to_string(),
        "BW3003"
    );
}

#[test]
fn listing_completion_and_hover_reuse_registered_metadata_and_normalization() {
    let mut context = Context::default();
    context.init_statements();
    context
        .register_native_with_signature(
            typed("Read |value|", ValueKind::String, ValueKind::String),
            |arguments| Ok(arguments[0].clone()),
        )
        .unwrap();
    run(
        "Render |value| { Return |value| }\nÅngström {}\nவணக்கம் {}",
        &mut context,
    )
    .unwrap();
    let names = context
        .statement_signatures()
        .into_iter()
        .map(|signature| signature.normalized().to_owned())
        .collect::<Vec<_>>();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(
        context
            .complete_statements(" R E ")
            .iter()
            .map(|signature| signature.normalized())
            .collect::<Vec<_>>(),
        ["read|param|", "render|param|"]
    );
    assert_eq!(context.complete_statements("வண").len(), 1);
    assert_eq!(context.complete_statements("ÅNG").len(), 1);
    assert_eq!(context.complete_statements("Read |typed").len(), 1);
    assert!(context.complete_statements("unregistered").is_empty());
    let program = Program::parse("hover.botwork", "READ |\"file\"|").unwrap();
    let StatementKind::Invoke(call) = program.statements[0].kind() else {
        panic!("call")
    };
    assert_eq!(
        context.signature_for_call(call).unwrap().help(),
        context
            .statement_signature("Read |x|")
            .unwrap()
            .unwrap()
            .help()
    );
    assert!(context.statement_signature("Unknown").unwrap().is_none());
    assert!(context.statement_signature("Read |1 + 2|").is_err());
}

fn all_values() -> [Literal; 7] {
    [
        Literal::None,
        Literal::Int(1),
        Literal::Float(1.0),
        Literal::Bool(true),
        Literal::String("text".into()),
        Literal::Array(vec![Literal::None]),
        Literal::Map(Default::default()),
    ]
}

#[test]
fn all_49_argument_kind_combinations_enforce_the_declared_contract() {
    for required in ValueKind::ALL {
        for value in all_values() {
            let kind = value.kind();
            let calls = Arc::new(AtomicUsize::new(0));
            let count = Arc::clone(&calls);
            let mut context = Context::default();
            context
                .register_native("Value", move |_| Ok(value.clone()))
                .unwrap();
            context
                .register_native_with_signature(
                    typed("Echo |value|", required, required),
                    move |values| {
                        count.fetch_add(1, Ordering::SeqCst);
                        Ok(values[0].clone())
                    },
                )
                .unwrap();
            let result = run("|value| = Value\nEcho |value|", &mut context);
            if kind == required {
                assert_eq!(result.unwrap().kind(), kind);
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            } else {
                assert_eq!(result.unwrap_err().code(), DiagnosticCode::IncompatibleType);
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        }
    }
}

#[test]
fn all_49_return_kind_combinations_enforce_the_declared_contract() {
    for required in ValueKind::ALL {
        for value in all_values() {
            let kind = value.kind();
            let mut context = Context::default();
            context
                .register_native_with_signature(
                    StatementSignature::native("Value")
                        .unwrap()
                        .returns(required),
                    move |_| Ok(value.clone()),
                )
                .unwrap();
            let result = run("|answer| = |99|\n|answer| = Value", &mut context);
            if kind == required {
                assert_eq!(result.unwrap().kind(), kind);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.code(), DiagnosticCode::IncompatibleType);
                assert_eq!(error.call_stack[0].signature, "value");
                assert!(matches!(
                    run("|result| = |answer|", &mut context),
                    Ok(Literal::Int(99))
                ));
            }
        }
    }
}
