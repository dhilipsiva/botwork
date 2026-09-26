#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::DiagnosticCode,
    grammar::{BWParser, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    syntax_limits::{
        SyntaxLimits, DEFAULT_SOURCE_BYTES, MAX_EXPRESSION_OPERATORS, MAX_SYNTAX_NESTING,
    },
};
use cli_harness::Harness;
use pest::Parser;
use std::{fs, time::Duration};

fn failure(source: &str, resource: &str) {
    let error = Program::parse_detailed("bounded.botwork", source).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit, "{error}");
    assert!(error.to_string().contains(resource));
    let span = error.span.unwrap();
    assert_eq!(span.source().name(), "bounded.botwork");
    assert!(source.is_char_boundary(span.start()) && source.is_char_boundary(span.end()));
    assert_eq!(span.source().text(), &source[..span.end()]);
}

#[test]
fn configured_limits_can_tighten_but_cannot_raise_parser_stack_ceilings() {
    for limits in [
        SyntaxLimits {
            nesting: MAX_SYNTAX_NESTING + 1,
            operators: 0,
        },
        SyntaxLimits {
            nesting: 0,
            operators: MAX_EXPRESSION_OPERATORS + 1,
        },
    ] {
        assert_eq!(
            Program::parse_bounded("config", "", 0, &limits)
                .unwrap_err()
                .code(),
            DiagnosticCode::RunConfiguration
        );
    }
    Program::parse_bounded(
        "empty",
        "",
        0,
        &SyntaxLimits {
            nesting: 0,
            operators: 0,
        },
    )
    .unwrap();
    assert!(Program::parse_bounded(
        "bounded",
        "Log |1|",
        100,
        &SyntaxLimits {
            nesting: 0,
            operators: 0
        }
    )
    .is_err());
    Program::parse_bounded(
        "bounded",
        "Log |1|",
        100,
        &SyntaxLimits {
            nesting: 1,
            operators: 0,
        },
    )
    .unwrap();
    assert!(Program::parse_bounded(
        "bounded",
        "Log |1+1|",
        100,
        &SyntaxLimits {
            nesting: 1,
            operators: 0
        }
    )
    .is_err());
}

#[test]
fn source_bytes_are_checked_before_parsing_copying_or_invalid_control_validation() {
    let text = format!("#{}🙂\nBreak", "x".repeat(DEFAULT_SOURCE_BYTES - 2));
    failure(&text, "source bytes");
    let error = Program::parse_detailed("big", &text).unwrap_err();
    assert!(error.span.as_ref().unwrap().source().text().len() <= DEFAULT_SOURCE_BYTES + 4);
    let larger = format!("#{}", "x".repeat(DEFAULT_SOURCE_BYTES));
    assert!(
        Program::parse_bounded("large", &larger, larger.len(), &SyntaxLimits::default()).is_ok()
    );
}

#[test]
fn excessive_arrays_maps_parentheses_blocks_and_call_expressions_fail_before_parser_entry() {
    for (open, close, atom) in [
        ("[", "]", "1"),
        ("{a:", "}", "1"),
        ("(", ")", "1"),
        ("@{ Identity |", "| }", "1"),
    ] {
        let text = format!("Log |{}{atom}{}|", open.repeat(10000), close.repeat(10000));
        failure(&text, "syntax nesting");
    }
    failure(
        &format!("{}{}", "If |true| {".repeat(10000), "}".repeat(10000)),
        "syntax nesting",
    );
}

#[test]
fn operator_chains_cannot_evade_limits_with_lines_comments_or_nested_call_arguments() {
    for source in [
        format!("Log |{}1|", "-\n# comment\n".repeat(1000)),
        format!("Log |{}1|", "1 ^ \n".repeat(1000)),
        format!("Log |{}1|", "1 + \n".repeat(1000)),
        format!("Log |{}true|", "true and\n".repeat(1000)),
        format!("Log |{}1|", "@{ Identity |1| } + ".repeat(1000)),
    ] {
        failure(
            &source,
            if source.contains("Identity") {
                "combined syntax complexity"
            } else {
                "expression operators"
            },
        );
    }
}

#[test]
fn strings_comments_and_sentence_quotes_have_grammar_compatible_boundaries() {
    let literal = format!("Log |\"{}\"|", "[({+-and Else If }])".repeat(1000));
    Program::parse_detailed("literal", &literal).unwrap();
    Program::parse_detailed(
        "comment",
        &format!("###{}###\nLog |1|", "{[(-Else If|".repeat(1000)),
    )
    .unwrap();
    Program::parse_detailed("escaped", "Log |\"\\\" ] } \\\\\"|\nLog |1|").unwrap();
    // Sentence text does not give quotes string semantics. Hiding braces here
    // would bypass a guard that treated every quote as a literal opener.
    failure(
        &format!("{}{}", "Name \" {\n".repeat(100), "}\n".repeat(100)),
        "syntax nesting",
    );
}

#[test]
fn else_if_chains_count_with_nested_blocks_but_independent_chains_reset() {
    let chain = format!(
        "If |false| {{}}{}",
        " Else\n### x ### If |false| {}".repeat(1000)
    );
    failure(&chain, "syntax nesting");
    Program::parse_detailed(
        "independent",
        &"If |false| {} Else If |true| {}\n".repeat(1000),
    )
    .unwrap();
    let nested = format!("If |true| {{\n{}\n}}", chain);
    failure(&nested, "syntax nesting");
}

#[test]
fn public_pest_parser_guards_standalone_expressions_and_programs_locally() {
    for rule in [
        Rule::expression,
        Rule::expression_inner,
        Rule::unary,
        Rule::power,
        Rule::primary,
    ] {
        let error = BWParser::parse(rule, &format!("{}1", "-\n".repeat(1000))).unwrap_err();
        assert!(error.to_string().contains("Resource limit exceeded"));
    }
    assert!(BWParser::parse(Rule::botwork, &format!("Log |{}1|", "(".repeat(1000))).is_err());
    BWParser::parse(Rule::expression, "1 + 2").unwrap();
}

#[test]
fn maximum_default_expression_and_delimiter_depths_parse_on_the_test_thread() {
    for (open, close) in [("(", ")"), ("[", "]"), ("{a:", "}")] {
        let depth = MAX_SYNTAX_NESTING - 1;
        Program::parse_detailed(
            "boundary",
            &format!("Log |{}1{}|", open.repeat(depth), close.repeat(depth)),
        )
        .unwrap();
    }
    for operator in ["1 + ", "1 ^ ", "-"] {
        Program::parse_detailed(
            "boundary",
            &format!("Log |{}1|", operator.repeat(MAX_EXPRESSION_OPERATORS)),
        )
        .unwrap();
    }
    Program::parse_detailed(
        "blocks",
        &format!(
            "{}{}",
            "If |true| {".repeat(MAX_SYNTAX_NESTING),
            "}".repeat(MAX_SYNTAX_NESTING)
        ),
    )
    .unwrap();
}

#[test]
fn cli_rejects_adversarial_sources_before_output_and_debug_traces() {
    let harness = Harness::new();
    for (id, source) in [
        (
            "nested",
            format!(
                "Log |\"unreachable\"|\nLog |{}1{}|",
                "[".repeat(10000),
                "]".repeat(10000)
            ),
        ),
        (
            "operators",
            format!("Log |\"unreachable\"|\nLog |{}1|", "-".repeat(10000)),
        ),
        (
            "source",
            format!(
                "Log |\"unreachable\"|\n#{}",
                "x".repeat(DEFAULT_SOURCE_BYTES)
            ),
        ),
    ] {
        let output = harness
            .run_with_args(id, &source, &["--debug"], Duration::from_secs(5))
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{id}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("BW8001") && error.contains(&format!("{id}.botwork")),
            "{error}"
        );
        assert!(!error.contains("debug:") && !error.contains("panicked"));
    }
    let output = harness
        .run("recovery", "Log |7|", Duration::from_secs(5))
        .unwrap();
    assert_eq!(output.stdout, b"7\n");
}

#[test]
fn engine_limits_apply_to_reused_programs_and_imported_source_without_running_handlers() {
    let engine = Engine::default();
    let program = Program::parse_detailed("program", "|x| = |1 + 2|").unwrap();
    let limits = RunLimits {
        syntax: SyntaxLimits {
            nesting: 1,
            operators: 0,
        },
        ..RunLimits::default()
    };
    let run = engine.run_program(
        &program,
        RunOptions {
            limits: limits.clone(),
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 0);
    let harness = Harness::new();
    fs::write(
        harness.workspace.join("module.botwork"),
        "Log |\"unreachable\"|\n|x| = |1+2|",
    )
    .unwrap();
    let run = engine.run_source(
        "main.botwork",
        "Try { Import |\"module.botwork\"| As |module| } Catch { |handled| = |true| }",
        RunOptions {
            working_directory: Some(harness.workspace.clone()),
            limits: RunLimits {
                syntax: SyntaxLimits {
                    operators: 0,
                    ..SyntaxLimits::default()
                },
                ..limits
            },
            ..RunOptions::default()
        },
    );
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(!run.variables.contains_key("handled"));
    let error = run.result.unwrap_err();
    assert!(error
        .span
        .as_ref()
        .unwrap()
        .source()
        .name()
        .ends_with("module.botwork"));
    assert_eq!(error.related.len(), 1);
}

#[test]
fn quote_in_call_sentence_cannot_hide_nested_argument_syntax() {
    let source = format!(
        "Log |@{{ Name \" |{}1{}| }}|",
        "(".repeat(1000),
        ")".repeat(1000)
    );
    failure(&source, "syntax nesting");
    let run = Engine::default().run_source(
        "quotes",
        "Name \" |x| { Return |x| }\n|result| = |@{ Name \" |7| }|",
        RunOptions::default(),
    );
    assert_eq!(run.result.unwrap().to_string(), "7");
}

#[test]
fn combined_maximum_guards_and_concurrent_tightening_preserve_default_behavior() {
    let source = format!(
        "Log |{}{}1{}|",
        "(".repeat(MAX_SYNTAX_NESTING - 1),
        "-".repeat(MAX_EXPRESSION_OPERATORS),
        ")".repeat(MAX_SYNTAX_NESTING - 1)
    );
    failure(&source, "combined syntax complexity");
    let workers: Vec<_> = [0, 1]
        .into_iter()
        .map(|operators| {
            std::thread::spawn(move || {
                let engine = Engine::default();
                engine
                    .run_source(
                        "run",
                        "|x| = |1+1|",
                        RunOptions {
                            limits: RunLimits {
                                syntax: SyntaxLimits {
                                    operators,
                                    ..SyntaxLimits::default()
                                },
                                ..RunLimits::default()
                            },
                            ..RunOptions::default()
                        },
                    )
                    .outcome()
            })
        })
        .collect();
    assert_eq!(
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>(),
        vec![RunOutcome::LimitExceeded, RunOutcome::Succeeded]
    );
    Program::parse_detailed("default", "|x| = |1+1|").unwrap();
}

#[test]
fn every_boundary_combination_of_delimiters_and_operator_depth_parses() {
    use botwork::core::syntax_limits::MAX_SYNTAX_COMPLEXITY;
    for (open, close, weight) in [
        ("(", ")", 1),
        ("[", "]", 1),
        ("{a:", "}", 1),
        ("@{ Identity |", "| }", 2),
    ] {
        for depth in 0..MAX_SYNTAX_NESTING {
            let nesting = depth * weight + 1;
            if nesting > MAX_SYNTAX_NESTING {
                break;
            }
            let operators = MAX_EXPRESSION_OPERATORS.min(MAX_SYNTAX_COMPLEXITY - 2 * nesting);
            for operator in ["-", "1 ^ ", "1 + "] {
                let source = format!(
                    "Log |{}{}1{}|",
                    open.repeat(depth),
                    operator.repeat(operators),
                    close.repeat(depth)
                );
                Program::parse_detailed("combination", &source).unwrap();
            }
        }
    }
    for depth in 0..MAX_SYNTAX_NESTING {
        let operators = MAX_EXPRESSION_OPERATORS.min(MAX_SYNTAX_COMPLEXITY - 2 * (depth + 1));
        let source = format!(
            "{}Log |{}1|{}",
            "If |true| {".repeat(depth),
            "-".repeat(operators),
            "}".repeat(depth)
        );
        Program::parse_detailed("blocks", &source).unwrap();
        let source = format!(
            "If |false| {{}}{} Else If |{}true| {{}}",
            " Else If |false| {}".repeat(depth.saturating_sub(1)),
            "!".repeat(operators.saturating_sub(2))
        );
        Program::parse_detailed("branches", &source).unwrap();
    }
}

#[test]
fn nonrecursive_pest_rules_treat_terminal_text_as_data() {
    for (rule, source) in [
        (Rule::string, format!("\"{}\"", "{[(+-Else If".repeat(1000))),
        (Rule::string_content, "{[(+-Else If".repeat(1000)),
        (Rule::part, "word ( [+ - ".repeat(1000)),
        (
            Rule::comment_block,
            format!("###{}###", "{[(+-Else If".repeat(1000)),
        ),
    ] {
        BWParser::parse(rule, &source).unwrap();
    }
}
