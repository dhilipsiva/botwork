//! Combined execution-order and cleanup matrices. The recorder is test-only.

use super::*;

fn run(source: &str, context: &mut Context) -> LiteralResult {
    let program = Program::parse("execution-contract.botwork", source).unwrap();
    evaluate_program(&program, context)
}

fn record(values: &[Literal], context: &mut Context) -> RuntimeResult {
    let value = values[0].clone();
    let depth = context.frames.len() as i32;
    for (key, entry) in [
        ("__events", Literal::String(value.to_string())),
        ("__depths", Literal::Int(depth)),
    ] {
        let Literal::Array(mut events) = context.frames[0].variables[key].value.clone() else {
            panic!("test event array");
        };
        events.push(entry);
        let stored = context.store_value(Literal::Array(events)).unwrap();
        context.frames[0].variables.insert(key.into(), stored);
    }
    Ok(value)
}

pub(super) fn context() -> Context {
    let mut context = Context::default();
    for key in ["__events", "__depths"] {
        context.set_variable(key, Literal::Array(vec![])).unwrap();
    }
    context
        .register_callback("<test Record>", "Record |value|", Arc::new(record))
        .unwrap();
    context
}

pub(super) fn events(context: &Context) -> Vec<String> {
    let Literal::Array(values) = context.get_variable("__events").unwrap() else {
        panic!("event array");
    };
    values
        .into_iter()
        .map(|value| match value {
            Literal::String(value) => value,
            _ => panic!("recorded string"),
        })
        .collect()
}

pub(super) fn visits(context: &Context) -> Vec<String> {
    context
        .expression_visits
        .borrow()
        .iter()
        .map(|text| text.trim().to_owned())
        .collect()
}

#[test]
fn every_argument_runs_once_in_caller_order_before_any_body_effect() {
    let setup = "|a| = |1|\n|b| = |2|\n|c| = |3|\n\
        Triple |a| with |b| and |c| { Record |[a, b, c]| }";
    for failure in [None, Some(0), Some(1), Some(2)] {
        let mut context = context();
        run(setup, &mut context).unwrap();
        context.expression_visits.borrow_mut().clear();
        let mut arguments = ["a", "b", "c"];
        if let Some(index) = failure {
            arguments[index] = "missing";
        }
        let source = format!(
            "Triple |{}| with |{}| and |{}|",
            arguments[0], arguments[1], arguments[2]
        );
        let result = run(&source, &mut context);
        match failure {
            Some(index) => {
                assert!(
                    matches!(result, Err(BWErr::VariableNotDefined(name)) if name == "missing")
                );
                assert_eq!(visits(&context), arguments[..=index]);
                assert!(events(&context).is_empty());
            }
            None => {
                assert!(matches!(result, Ok(Literal::None)));
                assert_eq!(
                    visits(&context),
                    ["a", "b", "c", "[a, b, c]", "a", "b", "c"]
                );
                assert_eq!(events(&context), ["[1, 2, 3]"]);
            }
        }
        assert_eq!(context.current, 0);
        assert_eq!(context.frames.len(), 1);
        for (name, expected) in [("a", 1), ("b", 2), ("c", 3)] {
            assert!(
                matches!(context.get_variable(name), Ok(Literal::Int(value)) if value == expected)
            );
        }
    }
    let mut context = context();
    assert!(matches!(
        run("Unknown |missing|", &mut context),
        Err(BWErr::StatementNotDefined(_))
    ));
    assert!(visits(&context).is_empty());
    assert!(events(&context).is_empty());
}

#[test]
fn nested_collection_entries_keep_source_order_even_for_overwritten_keys() {
    for failing in [
        None,
        Some("first"),
        Some("second"),
        Some("third"),
        Some("fourth"),
        Some("fifth"),
    ] {
        let mut context = context();
        let names = ["first", "second", "third", "fourth", "fifth"];
        for (index, name) in names.iter().enumerate() {
            if Some(*name) != failing {
                context
                    .set_variable(name, Literal::Int(index as i32 + 1))
                    .unwrap();
            }
        }
        context.set_variable("answer", Literal::Int(99)).unwrap();
        let result = run(
            "|answer| = |[first, {z: second, a: third, z: fourth}, fifth]|",
            &mut context,
        );
        let observed: Vec<_> = visits(&context)
            .into_iter()
            .filter(|name| names.contains(&name.as_str()))
            .collect();
        let expected_count = failing.map_or(5, |name| {
            names.iter().position(|item| *item == name).unwrap() + 1
        });
        assert_eq!(observed, names[..expected_count]);
        if let Some(name) = failing {
            assert!(matches!(result, Err(BWErr::VariableNotDefined(ref actual)) if actual == name));
            assert!(matches!(
                context.get_variable("answer"),
                Ok(Literal::Int(99))
            ));
        } else {
            assert_eq!(result.unwrap().to_string(), "[1, {\"a\": 3, \"z\": 4}, 5]");
        }
    }
}

#[test]
fn branch_conditions_run_once_and_only_until_a_branch_is_selected() {
    for (first, second, expected, conditions) in [
        (true, true, "first", vec!["first"]),
        (true, false, "first", vec!["first"]),
        (false, true, "second", vec!["first", "second"]),
        (false, false, "else", vec!["first", "second"]),
    ] {
        let mut context = context();
        context.set_variable("first", Literal::Bool(first)).unwrap();
        context
            .set_variable("second", Literal::Bool(second))
            .unwrap();
        run("If |first| { Record |\"first\"| } Else If |second| { Record |\"second\"| } Else { Record |\"else\"| }", &mut context).unwrap();
        let observed: Vec<_> = visits(&context)
            .into_iter()
            .filter(|name| ["first", "second"].contains(&name.as_str()))
            .collect();
        assert_eq!(observed, conditions);
        assert_eq!(events(&context), [expected]);
    }
}

#[test]
fn nested_for_bindings_restore_before_handlers_and_on_every_completion_path() {
    // Four binding states × seven outcomes × two control origins = 56 cases.
    for prior in ["absent", "none", "local", "inherited"] {
        for action in [
            "normal", "continue", "break", "return", "bare", "caught", "error",
        ] {
            for in_handler in [false, true] {
                let mut context = context();
                context.set_variable("item", Literal::Int(99)).unwrap();
                let mut frame = Frame {
                    parent: Some(0),
                    ..Frame::default()
                };
                match prior {
                    "none" => {
                        frame
                            .variables
                            .insert("item".into(), context.store_value(Literal::None).unwrap());
                    }
                    "local" => {
                        frame
                            .variables
                            .insert("item".into(), context.store_value(Literal::Int(7)).unwrap());
                    }
                    "absent" => {
                        context.frames[0].variables.remove("item");
                    }
                    _ => (),
                }
                context.frames.push(frame);
                context.current = 1;
                let control = match action {
                    "normal" => "",
                    "continue" => "Continue",
                    "break" => "Break",
                    "return" => "Return |42|",
                    "bare" => "Return\n",
                    _ => "|failure| = |missing_body|",
                };
                let transfer = if in_handler {
                    format!("Try {{ |trigger| = |missing_trigger| }} Catch {{ If |true| {{ {control} }} }}")
                } else {
                    format!("If |true| {{ {control} }}")
                };
                let handler_tail = if action == "error" {
                    "|failure| = |missing_handler|"
                } else {
                    ""
                };
                let source = format!(
                    r#"Holder {{
                    For |item| In |[1, 2]| {{
                        Record |["outer", item]|
                        Try {{
                            For |item| In |[10, 20]| {{
                                Record |["inner", item]|
                                {transfer}
                                Record |["tail", item]|
                            }}
                        }} Catch {{
                            Record |["caught", item]|
                            {handler_tail}
                        }}
                        Record |["after", item]|
                    }}
                }}"#
                );
                let program = Program::parse("nested.botwork", &source).unwrap();
                let StatementKind::Define(definition) = program.statements[0].kind() else {
                    panic!("holder")
                };
                // Inspect restored bindings before an invocation frame could mask a leak.
                let result = evaluate_block(&definition.body, &mut context)
                    .map_err(|error| error.into_diagnostic().into_error());
                assert!(
                    match action {
                        "return" =>
                            matches!(&result, Ok(Completion::Return(value)) if matches!(&**value, Literal::Int(42))),
                        "bare" =>
                            matches!(&result, Ok(Completion::Return(value)) if matches!(&**value, Literal::None)),
                        "error" =>
                            matches!(&result, Err(BWErr::VariableNotDefined(name)) if name == "missing_handler"),
                        _ =>
                            matches!(&result, Ok(Completion::Normal(value)) if matches!(&**value, Literal::None)),
                    },
                    "{prior}/{action}/{in_handler}: {result:?}"
                );
                let mut expected = vec![];
                for outer in 1..=2 {
                    expected.push(format!("[\"outer\", {outer}]"));
                    expected.push("[\"inner\", 10]".into());
                    if ["return", "bare"].contains(&action) {
                        break;
                    }
                    if ["caught", "error"].contains(&action) {
                        expected.push(format!("[\"caught\", {outer}]"));
                        if action == "error" {
                            break;
                        }
                    } else if action != "break" {
                        if action == "normal" {
                            expected.push("[\"tail\", 10]".into());
                        }
                        expected.push("[\"inner\", 20]".into());
                        if action == "normal" {
                            expected.push("[\"tail\", 20]".into());
                        }
                    }
                    expected.push(format!("[\"after\", {outer}]"));
                }
                assert_eq!(events(&context), expected, "{prior}/{action}/{in_handler}");
                let restored = context.get_variable("item");
                assert!(
                    match prior {
                        "absent" => matches!(restored, Err(BWErr::VariableNotDefined(_))),
                        "none" => matches!(restored, Ok(Literal::None)),
                        "local" => matches!(restored, Ok(Literal::Int(7))),
                        _ => matches!(restored, Ok(Literal::Int(99))),
                    },
                    "{prior}/{action}: {restored:?}"
                );
                assert_eq!(
                    context.frames[1].variables.contains_key("item"),
                    ["local", "none"].contains(&prior)
                );
                assert_eq!(context.current, 1);
                assert_eq!(context.frames.len(), 2);
            }
        }
    }
}

#[test]
fn while_rechecks_conditions_after_normal_or_continue_but_not_other_exits() {
    for (action, checks, expected) in [
        ("", 4, vec!["1", "2", "3"]),
        ("Continue", 4, vec!["1", "2", "3"]),
        ("Break", 1, vec!["1"]),
        ("Return |7|", 1, vec!["1"]),
        ("Return\n", 1, vec!["1"]),
        ("|failure| = |missing|", 1, vec!["1"]),
        ("|step| = |\"invalid\"|", 2, vec!["1"]),
    ] {
        let mut context = context();
        let source = format!("Work {{ |step| = |0|\nWhile |step < 3| {{\n|step| = |step + 1|\nRecord |step|\n{action}\n}} }}\nWork");
        let result = run(&source, &mut context);
        assert_eq!(
            result.is_err(),
            action.contains("missing") || action.contains("invalid"),
            "{action}: {result:?}"
        );
        assert_eq!(events(&context), expected, "{action}");
        assert_eq!(
            visits(&context)
                .iter()
                .filter(|text| *text == "step < 3")
                .count(),
            checks,
            "{action}"
        );
        assert_eq!(context.frames.len(), 1);
        assert_eq!(context.current, 0);
        assert!(context.get_variable("step").is_err());
    }
}

#[test]
fn recursive_traces_preserve_each_frame_and_unwind_before_the_callers_handler() {
    for fail in [false, true] {
        let mut context = context();
        let base = if fail {
            "Return |missing|"
        } else {
            "Return |0|"
        };
        run(
            &format!(
                r#"
            |n| = |99|
            Walk |n| {{
                Read local {{ Return |n| }}
                Record |["enter", n]|
                If |n == 0| {{ {base} }}
                |child| = Walk |n - 1|
                |own| = Read local
                Record |["leave", own]|
                Return |child + own|
            }}
        "#
            ),
            &mut context,
        )
        .unwrap();
        for _ in 0..2 {
            context
                .set_variable("__events", Literal::Array(vec![]))
                .unwrap();
            context
                .set_variable("__depths", Literal::Array(vec![]))
                .unwrap();
            run(
                "Try { |answer| = Walk |3| } Catch { Record |[\"caught\", n]| }",
                &mut context,
            )
            .unwrap();
            let mut expected: Vec<_> = (0..=3).rev().map(|n| format!("[\"enter\", {n}]")).collect();
            let expected_depths = if fail {
                expected.push("[\"caught\", 99]".into());
                assert!(context.get_variable("answer").is_err());
                "[2, 3, 4, 5, 1]"
            } else {
                expected.extend((1..=3).map(|n| format!("[\"leave\", {n}]")));
                assert!(matches!(
                    context.get_variable("answer"),
                    Ok(Literal::Int(6))
                ));
                "[2, 3, 4, 5, 4, 3, 2]"
            };
            assert_eq!(events(&context), expected);
            assert_eq!(
                context.get_variable("__depths").unwrap().to_string(),
                expected_depths
            );
            assert!(matches!(context.get_variable("n"), Ok(Literal::Int(99))));
            for local in ["own", "child"] {
                assert!(context.get_variable(local).is_err());
            }
            assert!(context.get_statement("readlocal").is_none());
            assert_eq!(context.frames.len(), 1);
            assert_eq!(context.current, 0);
        }
    }
}

#[test]
fn catch_bindings_and_handler_state_restore_on_every_completion_before_frame_disposal() {
    for prior in ["absent", "none", "local", "inherited"] {
        for (action, ending) in [
            ("normal", "|error| = |0|"),
            ("continue", "Continue"),
            ("break", "Break"),
            ("return", "Return |error.code|"),
            ("bare", "Return\n"),
            ("error", "|x| = |missing_handler|"),
            ("return-error", "Return |missing_return|"),
            ("rethrow", "Rethrow"),
        ] {
            let mut context = context();
            context.set_variable("error", Literal::Int(99)).unwrap();
            let mut frame = Frame {
                parent: Some(0),
                ..Frame::default()
            };
            match prior {
                "none" => {
                    frame
                        .variables
                        .insert("error".into(), context.store_value(Literal::None).unwrap());
                }
                "local" => {
                    frame.variables.insert(
                        "error".into(),
                        context.store_value(Literal::Int(7)).unwrap(),
                    );
                }
                "absent" => {
                    context.frames[0].variables.remove("error");
                }
                _ => (),
            }
            context.frames.push(frame);
            context.current = 1;
            let source = format!(
                "Holder {{ For |item| In |[1]| {{\n\
                Try {{ |x| = |missing_body| }} Catch |error| {{\n\
                Record |error.code|\n{ending}\n}}\n}} }}"
            );
            let program = Program::parse("catch-cleanup.botwork", &source).unwrap();
            let StatementKind::Define(definition) = program.statements[0].kind() else {
                panic!("holder")
            };
            let StatementKind::For { body, .. } = definition.body.statements[0].kind() else {
                panic!("loop")
            };
            // Inspect the handler exit before the loop or invocation can consume it.
            let result = evaluate_statement(&body.statements[0], &mut context);
            assert!(
                match action {
                    "normal" =>
                        matches!(&result, Ok(Completion::Normal(value)) if matches!(&**value, Literal::None)),
                    "continue" => matches!(result, Ok(Completion::Continue)),
                    "break" => matches!(result, Ok(Completion::Break)),
                    "return" =>
                        matches!(&result, Ok(Completion::Return(value)) if matches!(&**value, Literal::String(code) if code == "BW2001")),
                    "bare" =>
                        matches!(&result, Ok(Completion::Return(value)) if matches!(&**value, Literal::None)),
                    _ => matches!(&result, Err(error) if error.code().as_str() == "BW2001"),
                },
                "{prior}/{action}: {result:?}"
            );
            if let Err(error) = &result {
                assert_eq!(error.causes.len(), usize::from(action != "rethrow"));
                let expected = match action {
                    "rethrow" => "missing_body",
                    "return-error" => "missing_return",
                    _ => "missing_handler",
                };
                assert!(
                    matches!(error.error.as_ref(), BWErr::VariableNotDefined(name) if name == expected)
                );
            }
            assert_eq!(events(&context), ["BW2001"]);
            let restored = context.get_variable("error");
            assert!(
                match prior {
                    "absent" => matches!(restored, Err(BWErr::VariableNotDefined(_))),
                    "none" => matches!(restored, Ok(Literal::None)),
                    "local" => matches!(restored, Ok(Literal::Int(7))),
                    _ => matches!(restored, Ok(Literal::Int(99))),
                },
                "{prior}/{action}: {restored:?}"
            );
            assert_eq!(
                context.frames[1].variables.contains_key("error"),
                ["local", "none"].contains(&prior)
            );
            assert!(context.handlers.is_empty());
            assert!(context.calls.is_empty());
            assert_eq!(context.current, 1);
            assert_eq!(context.frames.len(), 2);
        }
    }
}

#[test]
fn defensive_rethrow_guard_does_not_consume_an_unrelated_callers_handler() {
    let mut context = context();
    context.handlers.push(HandledError {
        invocation: 0,
        diagnostic: context
            .retain_handler(Diagnostic::new(BWErr::VariableNotDefined(
                "original".into(),
            )))
            .unwrap(),
    });
    let program = Program::parse("guard.botwork", "Try {} Catch { Rethrow }").unwrap();
    let StatementKind::Try { handler, .. } = program.statements[0].kind() else {
        panic!("try")
    };
    let statement = &handler.statements[0];
    // Deliberately bypass public placement validation to check the runtime boundary.
    let error = context
        .with_invocation(
            Frame {
                parent: Some(0),
                ..Frame::default()
            },
            |context| evaluate_statement(statement, context),
        )
        .unwrap_err();
    assert!(matches!(error.error.as_ref(), BWErr::ControlFlowError(_)));
    assert_eq!(context.current, 0);
    assert_eq!(context.handlers.len(), 1);
    let error = evaluate_statement(statement, &mut context).unwrap_err();
    assert!(matches!(error.error.as_ref(), BWErr::VariableNotDefined(name) if name == "original"));
    assert!(error.causes.is_empty());
    assert_eq!(context.handlers.len(), 1);
}
