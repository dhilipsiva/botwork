use super::*;
use crate::core::{diagnostic::DiagnosticCode, syntax_limits::DEFAULT_SOURCE_BYTES};

fn span() -> Span {
    Program::parse("span", "|x| = |1|").unwrap().statements[0]
        .span
        .clone()
}

#[test]
fn node_count_includes_names_operator_spans_containers_and_call_wrappers() {
    for (source, nodes, depth) in [
        ("|x| = |1|", 3, 2),
        ("|x| = |-1|", 5, 3),
        ("|x| = |1+2|", 6, 3),
        ("Log |1|", 3, 3),
        ("|x| = |@{Log |1|}|", 5, 4),
        ("|x| = |[1,2]|", 5, 3),
        ("|x| = |{key:1}|", 5, 3),
        ("|x| = |values.key[0]|", 7, 4),
        ("Read |x| { Return |x| }", 7, 5),
    ] {
        let program = Program::parse("counts", source).unwrap();
        let exact = AstLimits {
            nodes,
            depth,
            ..AstLimits::default()
        };
        check_program(&program, &exact, DEFAULT_SOURCE_BYTES)
            .unwrap_or_else(|e| panic!("{source}: {e}"));
        let error = check_program(
            &program,
            &AstLimits {
                nodes: nodes - 1,
                ..exact.clone()
            },
            DEFAULT_SOURCE_BYTES,
        )
        .unwrap_err();
        assert!(error.to_string().contains("AST nodes"), "{source}: {error}");
        let error = check_program(
            &program,
            &AstLimits {
                depth: depth - 1,
                ..exact
            },
            DEFAULT_SOURCE_BYTES,
        )
        .unwrap_err();
        assert!(error.to_string().contains("AST depth"), "{source}: {error}");
    }
}

#[test]
fn deeply_assembled_expression_is_rejected_iteratively() {
    let location = span();
    let mut expression = Expr {
        span: location.clone(),
        kind: ExprKind::Bool(true),
    };
    for _ in 0..10_000 {
        expression = Expr {
            span: location.clone(),
            kind: ExprKind::Unary {
                operator: UnaryOp::Not,
                operator_span: location.clone(),
                operand: Box::new(expression),
            },
        };
    }
    let node = Node::Expression(expression);
    let error = check_node(&node, &AstLimits::default(), DEFAULT_SOURCE_BYTES).unwrap_err();
    assert!(error.to_string().contains("AST depth"));
    // The host owns rejected input, including its destruction strategy.
    let Node::Expression(mut expression) = node else {
        unreachable!()
    };
    while let ExprKind::Unary { operand, .. } = expression.kind {
        expression = *operand;
    }
}

#[test]
fn deep_host_control_tree_fails_before_recursive_control_validation() {
    let location = span();
    let mut statement = Statement {
        span: location.clone(),
        kind: StatementKind::Break,
    };
    for _ in 0..1000 {
        statement = Statement {
            span: location.clone(),
            kind: StatementKind::If {
                condition: Expr {
                    span: location.clone(),
                    kind: ExprKind::Bool(false),
                },
                then_branch: Block {
                    span: location.clone(),
                    statements: vec![statement],
                },
                else_branch: None,
            },
        };
    }
    let mut program = Program {
        source: Arc::clone(location.source()),
        statements: vec![statement],
    };
    assert_eq!(
        program.validate_detailed().unwrap_err().code(),
        DiagnosticCode::ResourceLimit
    );
    let mut statement = program.statements.pop().unwrap();
    while let StatementKind::If {
        mut then_branch, ..
    } = statement.kind
    {
        statement = then_branch.statements.pop().unwrap();
    }
}

#[test]
fn every_hidden_span_source_is_included_in_admission() {
    let small = Program::parse("small", "Read |x| { Return |x| }").unwrap();
    let big = Program::parse("big", &format!("{}\n|x| = |1|", "#".repeat(200))).unwrap();
    let big_span = &big.statements[0].span;
    let StatementKind::Define(original) = &small.statements[0].kind else {
        unreachable!()
    };
    for location in 0..5 {
        let mut definition = (**original).clone();
        match location {
            0 => definition.span = big_span.clone(),
            1 => definition.header = big_span.clone(),
            2 => definition.parameters[0].span = big_span.clone(),
            3 => definition.body.span = big_span.clone(),
            _ => definition.body.statements[0].span = big_span.clone(),
        }
        let program = Program {
            source: Arc::clone(&small.source),
            statements: vec![Statement {
                span: small.statements[0].span.clone(),
                kind: StatementKind::Define(Arc::new(definition)),
            }],
        };
        let error = check_program(&program, &AstLimits::default(), 100).unwrap_err();
        assert_eq!(error.span.as_ref().unwrap().source().name(), "big");
    }
}

#[test]
fn empty_node_and_zero_budgets_are_valid_but_invalid_configuration_is_not() {
    let zero = AstLimits {
        nodes: 0,
        depth: 0,
        source_bytes: 0,
    };
    check_node(&Node::None, &zero, 0).unwrap();
    let invalid = AstLimits {
        depth: MAX_AST_DEPTH + 1,
        ..zero
    };
    assert_eq!(
        check_node(&Node::None, &invalid, 0).unwrap_err().code(),
        DiagnosticCode::RunConfiguration
    );
}

#[test]
fn every_control_branch_is_checked_before_any_branch_is_selected() {
    for source in [
        "If |true| {} Else If |false| { |x| = |1| } Else {}",
        "For |x| In |[]| { Break\nContinue }",
        "While |false| { |x| = |1| }",
        "Try {} Catch |error| { Rethrow }",
        "Read { Return }",
        "Import |\"other.botwork\"| As |other|",
    ] {
        let program = Program::parse("branches", source).unwrap();
        check_program(&program, &AstLimits::default(), DEFAULT_SOURCE_BYTES).unwrap();
        assert!(check_program(
            &program,
            &AstLimits {
                nodes: 1,
                ..AstLimits::default()
            },
            DEFAULT_SOURCE_BYTES
        )
        .is_err());
    }
}

#[test]
fn admitted_control_depth_reaches_lexical_validation_at_the_boundary() {
    let location = span();
    for nested in [63, 64] {
        let mut statement = Statement {
            span: location.clone(),
            kind: StatementKind::Break,
        };
        for _ in 0..nested {
            statement = Statement {
                span: location.clone(),
                kind: StatementKind::If {
                    condition: Expr {
                        span: location.clone(),
                        kind: ExprKind::Bool(false),
                    },
                    then_branch: Block {
                        span: location.clone(),
                        statements: vec![statement],
                    },
                    else_branch: None,
                },
            };
        }
        let program = Program {
            source: Arc::clone(location.source()),
            statements: vec![statement],
        };
        let error = program.validate_detailed().unwrap_err();
        assert_eq!(
            error.code(),
            if nested == 63 {
                DiagnosticCode::InvalidControl
            } else {
                DiagnosticCode::ResourceLimit
            }
        );
    }
}
