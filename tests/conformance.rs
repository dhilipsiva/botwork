#[path = "support/cli_harness.rs"]
mod cli_harness;
#[path = "conformance/cases.rs"]
mod corpus;

use botwork::core::grammar::{BWErr, Literal, Operate, Rule};
use cli_harness::Harness;
use corpus::{cases, Case, Input};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    time::Duration,
};

const SPECIFICATION: &str = include_str!("../docs/language-specification.md");

fn rule_ids(specification: &str) -> Result<BTreeSet<&str>, String> {
    let mut rules = BTreeSet::new();
    for line in specification.lines() {
        if let Some((id, _)) = line
            .strip_prefix("**")
            .and_then(|line| line.split_once(" — "))
        {
            if !rules.insert(id) {
                return Err(format!("duplicate specification rule: {id}"));
            }
        }
    }
    if rules.is_empty() {
        return Err("specification has no rule IDs".into());
    }
    Ok(rules)
}

fn inventory(cases: &[Case], specification: &str) -> Result<(), String> {
    let rules = rule_ids(specification)?;
    let mut coverage: BTreeMap<_, [bool; 3]> = rules.iter().map(|id| (*id, [false; 3])).collect();
    let mut names = BTreeSet::new();
    for case in cases {
        if !names.insert(case.id) {
            return Err(format!("duplicate case: {}", case.id));
        }
        if case.error.is_some() != !case.invalid.is_empty() {
            return Err(format!("{}: invalid cases must expect an error", case.id));
        }
        if case.error.is_some() != case.code.is_some() {
            return Err(format!("{}: error expectations require a code", case.id));
        }
        let mut count = 0;
        for (category, ids) in [case.positive, case.invalid, case.boundary]
            .iter()
            .enumerate()
        {
            let mut seen = BTreeSet::new();
            for id in *ids {
                if !seen.insert(id) {
                    return Err(format!("{}: duplicate rule {id}", case.id));
                }
                let present = coverage
                    .get_mut(id)
                    .ok_or_else(|| format!("{}: unknown rule {id}", case.id))?;
                present[category] = true;
                count += 1;
            }
        }
        if count == 0 {
            return Err(format!("{}: case covers no rules", case.id));
        }
    }
    for (id, present) in coverage {
        for (index, category) in ["positive", "invalid", "boundary"].iter().enumerate() {
            if !present[index] {
                return Err(format!("{id}: missing {category} evidence"));
            }
        }
    }
    Ok(())
}

#[test]
fn corpus_covers_each_specified_rule_and_registers_every_fixture() {
    let cases = cases();
    inventory(&cases, SPECIFICATION).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "botwork")
        {
            let name = path.file_stem().unwrap().to_str().unwrap();
            let case = cases
                .iter()
                .find(|case| case.id == name)
                .unwrap_or_else(|| panic!("unregistered corpus fixture: {name}"));
            let Input::Script(source) = case.input else {
                panic!("script fixture must run through CLI")
            };
            assert_eq!(source, fs::read_to_string(path).unwrap());
        }
    }
}

#[test]
fn inventory_rejects_new_rules_gaps_unknown_tags_duplicates_and_wrong_expectations() {
    assert!(inventory(
        &cases(),
        &format!("{SPECIFICATION}\n**X1 — New rule.** Text.")
    )
    .unwrap_err()
    .contains("X1"));
    let mut missing = cases();
    missing.retain(|case| case.id != "structural-host-invalid");
    assert_eq!(
        inventory(&missing, SPECIFICATION).unwrap_err(),
        "V6: missing invalid evidence"
    );
    let mut unknown = cases();
    unknown[0].positive = &["X1"];
    assert!(inventory(&unknown, SPECIFICATION)
        .unwrap_err()
        .contains("unknown rule X1"));
    let mut duplicate = cases();
    duplicate.push(duplicate[0].clone());
    assert!(inventory(&duplicate, SPECIFICATION)
        .unwrap_err()
        .contains("duplicate case"));
    let mut repeated_rule = cases();
    repeated_rule[0].positive = &["E2", "E2"];
    assert!(inventory(&repeated_rule, SPECIFICATION)
        .unwrap_err()
        .contains("duplicate rule"));
    let mut wrong_status = cases();
    wrong_status[0].error = Some("unexpected");
    assert!(inventory(&wrong_status, SPECIFICATION).is_err());
    let mut missing_code = cases();
    missing_code.last_mut().unwrap().code = None;
    assert!(inventory(&missing_code, SPECIFICATION)
        .unwrap_err()
        .contains("require a code"));
    assert!(rule_ids("no rules").is_err());
    assert!(rule_ids("**E1 — First.**\n**E1 — Duplicate.**").is_err());
}

fn check_host_case(case: &Case) {
    // Source literals cannot produce non-finite values. Exercise the public Rust
    // operator API on both sides, including nested values and mismatched shapes.
    for number in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for invalid in [
            Literal::Float(number),
            Literal::Array(vec![Literal::Int(1), Literal::Float(number)]),
            Literal::Map(
                [(
                    "nested".into(),
                    Literal::Array(vec![Literal::Float(number)]),
                )]
                .into_iter()
                .collect(),
            ),
        ] {
            for operator in [Rule::equal, Rule::not_equal] {
                for (left, right) in [
                    (invalid.clone(), Literal::None),
                    (Literal::None, invalid.clone()),
                ] {
                    let error = operator.operate_binary(left, right).unwrap_err();
                    assert_eq!(error.code().as_str(), case.code.unwrap());
                    assert!(
                        matches!(error, BWErr::ArithmeticError(_)),
                        "{}: {error}",
                        case.id
                    );
                    assert!(error.to_string().contains(case.error.unwrap()));
                }
            }
        }
    }
}

fn check_native_case(case: &Case) {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    let value = match case.input {
        Input::NativeReturn => Literal::Array(vec![Literal::None, Literal::Int(i32::MIN)]),
        Input::NativeInvalidReturn => Literal::Array(vec![Literal::Float(f32::INFINITY)]),
        _ => unreachable!(),
    };
    context
        .register_native("Host", move |arguments| {
            assert!(arguments.is_empty());
            Ok(value.clone())
        })
        .unwrap();
    let result = evaluate_program_detailed(
        &Program::parse("native-corpus.botwork", "|result| = Host").unwrap(),
        &mut context,
    );
    match case.error {
        Some(expected) => {
            let error = result.unwrap_err();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert_eq!(error.call_stack[0].signature, "host");
        }
        None => assert_eq!(result.unwrap().to_string(), "[none, -2147483648]"),
    }
}

fn check_signature_case(case: &Case) {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        signature::{StatementSignature, ValueKind, ValueKinds},
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let count = Arc::new(AtomicUsize::new(0));
    let invoked = Arc::clone(&count);
    let mut context = Context::default();
    let signature = StatementSignature::native("Host |value|")
        .unwrap()
        .parameter(
            "value",
            ValueKinds::one(ValueKind::Int).union(ValueKind::Float.into()),
        )
        .unwrap()
        .returns(ValueKind::Int);
    let expected_help = signature.help();
    context
        .register_native_with_signature(signature, move |_| {
            invoked.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::Int(7))
        })
        .unwrap();
    assert_eq!(
        context
            .statement_signature("h o s t |other|")
            .unwrap()
            .unwrap()
            .help(),
        expected_help
    );
    let source = if case.error.is_some() {
        "Host |true|"
    } else {
        "Host |1.0|"
    };
    let result = evaluate_program_detailed(
        &Program::parse("signature-corpus.botwork", source).unwrap(),
        &mut context,
    );
    match case.error {
        Some(expected) => {
            let error = result.unwrap_err();
            assert_eq!(error.code().as_str(), case.code.unwrap());
            assert!(error.to_string().contains(expected));
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(error.call_stack.is_empty());
        }
        None => {
            assert!(matches!(result, Ok(Literal::Int(7))));
            assert_eq!(count.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn conformance_inputs_match_status_stdout_and_error_contracts() {
    let cases = cases();
    inventory(&cases, SPECIFICATION).unwrap();
    let harness = Harness::new();
    for case in cases {
        let source = match case.input {
            Input::Script(source) => source,
            Input::NonFiniteHost => {
                check_host_case(&case);
                continue;
            }
            Input::NativeReturn | Input::NativeInvalidReturn => {
                check_native_case(&case);
                continue;
            }
            Input::SignatureValid | Input::SignatureInvalid => {
                check_signature_case(&case);
                continue;
            }
        };
        let output = harness
            .run(case.id, source, Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("{error}"));
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        let label = format!(
            "{}; {}; timeout=5s\n{source}\n{diagnostic}",
            case.id, harness.environment
        );
        assert_eq!(output.stdout, case.stdout.as_bytes(), "{label}");
        match case.error {
            Some(expected) => {
                assert!(
                    diagnostic.contains(&format!("[{}]", case.code.unwrap())),
                    "{label}"
                );
                assert_eq!(output.status.code(), Some(1), "{label}");
                assert!(
                    diagnostic.contains(expected),
                    "expected {expected}: {label}"
                );
                assert!(
                    diagnostic.contains(&format!("{}.botwork", case.id)),
                    "{label}"
                );
                assert!(!diagnostic.contains("panicked"), "{label}");
            }
            None => {
                assert_eq!(output.status.code(), Some(0), "{label}");
                assert!(diagnostic.is_empty(), "{label}");
            }
        }
    }
}
