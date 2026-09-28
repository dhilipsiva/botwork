//! Version 1 deterministic input decoding and independent semantic oracles.
//! Shared by ordinary regression tests and libFuzzer; never executes raw fuzz DSL.
#![allow(dead_code)]
use botwork::core::{
    ast::{Program, Span},
    ast_limits::AstLimits,
    diagnostic::{Diagnostic, DiagnosticCode},
    grammar::Literal,
    run::{Engine, RunLimits, RunOptions},
    syntax_limits::SyntaxLimits,
};
use std::collections::HashMap;

pub const SOURCE_BYTES: usize = 4096;
pub const STEPS: u64 = 256;

fn span_is_valid(span: &Span) {
    let text = span.source().text();
    assert!(span.start() <= span.end() && span.end() <= text.len());
    assert!(text.is_char_boundary(span.start()) && text.is_char_boundary(span.end()));
    assert_eq!(span.text(), &text[span.start()..span.end()]);
    // Count scalar positions in a forward walk, including CRLF and tabs.
    for (offset, actual) in [
        (span.start(), span.line_column()),
        (span.end(), span.end_line_column()),
    ] {
        let (mut line, mut column) = (1, 1);
        for ch in text[..offset].chars() {
            if ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        assert_eq!(actual, (line, column));
    }
}

fn diagnostic_is_valid(error: &Diagnostic) {
    let mut pending = vec![error];
    while let Some(error) = pending.pop() {
        if let Some(span) = &error.span {
            span_is_valid(span);
        }
        for frame in &error.call_stack {
            span_is_valid(&frame.call_site);
            if let Some(span) = &frame.definition_site {
                span_is_valid(span);
            }
        }
        for related in &error.related {
            span_is_valid(&related.span);
        }
        pending.extend(&error.causes);
    }
    // Rendering rejected input is part of the public failure path.
    assert!(error.to_string().contains(error.code().as_str()));
}

/// Invalid UTF-8 is outside the &str parser API and is rejected by the harness.
/// Over-budget valid text still enters the public preflight rejection path.
pub fn parse(data: &[u8]) -> bool {
    let Ok(source) = std::str::from_utf8(data) else {
        return false;
    };
    let ast = AstLimits {
        nodes: 2048,
        depth: 96,
        source_bytes: SOURCE_BYTES,
    };
    let parse = || {
        Program::parse_with_budgets(
            "generated.botwork",
            source,
            SOURCE_BYTES,
            &SyntaxLimits::default(),
            &ast,
        )
    };
    match parse() {
        Ok(program) => {
            assert_eq!(program.source.text(), source);
            assert!(program.validate_with_limits(&ast, SOURCE_BYTES).is_ok());
            for statement in &program.statements {
                span_is_valid(&statement.span);
            }
            let repeated = parse().expect("same bounded parse must stay valid");
            assert_eq!(format!("{program:?}"), format!("{repeated:?}"));
            true
        }
        Err(error) => {
            diagnostic_is_valid(&error);
            let repeated = parse().expect_err("same bounded parse must stay invalid");
            assert_eq!(error.code(), repeated.code());
            assert_eq!(error.to_string(), repeated.to_string());
            false
        }
    }
}

struct Bytes<'a>(&'a [u8]);
impl Bytes<'_> {
    fn byte(&mut self) -> u8 {
        let Some((&first, rest)) = self.0.split_first() else {
            return 0;
        };
        self.0 = rest;
        first
    }
    fn word(&mut self) -> u32 {
        u32::from_le_bytes([self.byte(), self.byte(), self.byte(), self.byte()])
    }
    fn integer(&mut self) -> i32 {
        match self.byte() % 8 {
            0 => i32::MIN,
            1 => i32::MAX,
            2 => -1,
            3 => 0,
            4 => 1,
            _ => self.word() as i32,
        }
    }
    fn string(&mut self) -> String {
        let size = (self.byte() as usize % 33).min(self.0.len());
        let (text, rest) = self.0.split_at(size);
        self.0 = rest;
        String::from_utf8_lossy(text).into_owned()
    }
}

fn run(source: &str) -> botwork::core::run::RunResult {
    assert!(
        source.len() <= SOURCE_BYTES,
        "generator exceeded its source budget"
    );
    let result = Engine::default().run_source(
        "generated.botwork",
        source,
        RunOptions {
            inherit_environment: false,
            limits: RunLimits {
                source_bytes: SOURCE_BYTES,
                steps: STEPS,
                call_depth: 8,
                evaluation_depth: 64,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    assert!(result.steps <= STEPS);
    assert!(result.snapshot_error.is_none());
    result
}

#[derive(Clone, Copy)]
enum Operator {
    Add,
    Subtract,
    Multiply,
    Remainder,
}
impl Operator {
    fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Remainder => "%",
        }
    }
    fn high(self) -> bool {
        matches!(self, Self::Multiply | Self::Remainder)
    }
    fn calculate(self, left: i32, right: i32) -> Option<i32> {
        // Wide integer arithmetic plus explicit language bounds. This does not
        // use Literal operators, Pest, Pratt parsing, or the evaluator's helpers.
        let wide = match self {
            Self::Add => i128::from(left) + i128::from(right),
            Self::Subtract => i128::from(left) - i128::from(right),
            Self::Multiply => i128::from(left) * i128::from(right),
            Self::Remainder => {
                // The language specifies MIN % -1 == 0. Wide arithmetic
                // obtains this directly without Rust's narrow remainder trap.
                if right == 0 {
                    return None;
                }
                i128::from(left) % i128::from(right)
            }
        };
        i32::try_from(wide).ok()
    }
}

/// Compare a flat integer expression and an explicitly grouped spelling with
/// an independently calculated sum/difference of multiplicative terms.
pub fn expression(data: &[u8]) {
    let mut bytes = Bytes(data);
    let count = 1 + bytes.byte() % 8;
    let mut values = vec![bytes.integer()];
    let mut operators = Vec::new();
    for _ in 1..count {
        operators.push(match bytes.byte() % 4 {
            0 => Operator::Add,
            1 => Operator::Subtract,
            2 => Operator::Multiply,
            _ => Operator::Remainder,
        });
        values.push(bytes.integer());
    }
    let mut flat = values[0].to_string();
    for (op, value) in operators.iter().zip(&values[1..]) {
        flat.push_str(&format!(" {} {value}", op.symbol()));
    }
    let (mut terms, mut low) = (Vec::new(), Vec::new());
    let (mut value, mut spelling) = (Some(values[0]), values[0].to_string());
    for (op, next) in operators.iter().copied().zip(values[1..].iter().copied()) {
        if op.high() {
            value = value.and_then(|value| op.calculate(value, next));
            spelling = format!("({spelling} {} {next})", op.symbol());
        } else {
            terms.push((value, spelling));
            low.push(op);
            (value, spelling) = (Some(next), next.to_string());
        }
    }
    terms.push((value, spelling));
    let mut terms = terms.into_iter();
    let (mut expected, mut grouped) = terms.next().unwrap();
    for (op, (value, spelling)) in low.into_iter().zip(terms) {
        expected = expected
            .zip(value)
            .and_then(|(left, right)| op.calculate(left, right));
        grouped = format!("({grouped} {} {spelling})", op.symbol());
    }
    for expression in [flat, grouped] {
        let source = format!("|before| = |7|\n|answer| = |{expression}|\n|after| = |1|");
        let result = run(&source);
        assert!(matches!(
            result.variables.get("before"),
            Some(Literal::Int(7))
        ));
        match expected {
            Some(expected) => {
                assert!(result.result.is_ok(), "{source}: {:?}", result.result);
                assert!(
                    matches!(result.variables.get("answer"), Some(Literal::Int(actual)) if *actual == expected),
                    "{source}: expected {expected}, got {:?}",
                    result.variables
                );
                assert!(matches!(
                    result.variables.get("after"),
                    Some(Literal::Int(1))
                ));
            }
            None => {
                let error = result.result.expect_err(&source);
                assert_eq!(
                    error.code(),
                    DiagnosticCode::Arithmetic,
                    "{source}: {error}"
                );
                diagnostic_is_valid(&error);
                assert!(!result.variables.contains_key("answer"));
                assert!(!result.variables.contains_key("after"));
            }
        }
    }
}

fn quote(text: &str) -> String {
    let mut result = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            _ => result.push(ch),
        }
    }
    result.push('"');
    result
}

fn value(bytes: &mut Bytes<'_>, depth: usize) -> (String, Literal) {
    match bytes.byte() % if depth == 0 { 5 } else { 7 } {
        0 => ("@{ Empty }".into(), Literal::None),
        1 => {
            let value = bytes.integer();
            (value.to_string(), Literal::Int(value))
        }
        2 => {
            let bits = match bytes.byte() % 8 {
                0 => 0,
                1 => 1 << 31,
                2 => 1,
                3 => f32::MAX.to_bits(),
                4 => f32::MIN_POSITIVE.to_bits(),
                _ => {
                    let bits = bytes.word();
                    if bits & 0x7f800000 == 0x7f800000 {
                        bits & !0x00800000
                    } else {
                        bits
                    }
                }
            };
            let value = f32::from_bits(bits);
            assert!(value.is_finite());
            // Every binary32 value has an exact decimal with at most 149
            // fractional places. DSL float syntax has no exponent notation.
            (format!("{value:.149}"), Literal::Float(value))
        }
        3 => {
            let value = bytes.byte() & 1 != 0;
            (value.to_string(), Literal::Bool(value))
        }
        4 => {
            let value = bytes.string();
            (quote(&value), Literal::String(value))
        }
        5 => {
            let mut source = Vec::new();
            let mut values = Vec::new();
            for _ in 0..bytes.byte() % 3 {
                let (text, expected) = value(bytes, depth - 1);
                source.push(text);
                values.push(expected);
            }
            (format!("[{}]", source.join(",")), Literal::Array(values))
        }
        _ => {
            let mut source = Vec::new();
            let mut values = HashMap::new();
            for _ in 0..bytes.byte() % 3 {
                let key = match bytes.byte() % 4 {
                    0 => "".into(),
                    1 => "தமிழ்".into(),
                    2 => "e\u{301}".into(),
                    _ => bytes.string(),
                };
                let (text, expected) = value(bytes, depth - 1);
                source.push(format!("{}:{text}", quote(&key)));
                values.insert(key, expected);
            }
            (format!("{{{}}}", source.join(",")), Literal::Map(values))
        }
    }
}

fn equal_bits(actual: &Literal, expected: &Literal) {
    match (actual, expected) {
        (Literal::None, Literal::None) => (),
        (Literal::Int(a), Literal::Int(b)) => assert_eq!(a, b),
        (Literal::Float(a), Literal::Float(b)) => assert_eq!(a.to_bits(), b.to_bits()),
        (Literal::Bool(a), Literal::Bool(b)) => assert_eq!(a, b),
        (Literal::String(a), Literal::String(b)) => assert_eq!(a.as_bytes(), b.as_bytes()),
        (Literal::Array(a), Literal::Array(b)) => {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                equal_bits(a, b);
            }
        }
        (Literal::Map(a), Literal::Map(b)) => {
            assert_eq!(a.len(), b.len());
            for (key, b) in b {
                equal_bits(a.get(key).expect("generated key retained"), b);
            }
        }
        _ => panic!("different literal kinds: {actual:?} vs {expected:?}"),
    }
}

pub fn literals(data: &[u8]) {
    let (text, expected) = value(&mut Bytes(data), 3);
    let source = format!("Empty {{}}\n|answer| = |{text}|");
    let result = run(&source);
    assert!(result.result.is_ok(), "{source}: {:?}", result.result);
    equal_bits(result.variables.get("answer").unwrap(), &expected);
}

/// SplitMix64, with specified wrapping arithmetic: corpus bytes are independent
/// of Rust rand versions, host usize width, and debug/release overflow checks.
pub fn case_bytes(seed: u64, case: u64) -> Vec<u8> {
    // A different stride from SplitMix's transition prevents adjacent cases
    // from being shifted copies of the same 32-word window.
    let mut state = seed ^ case.wrapping_mul(0xd1b54a32d192ed03);
    let mut result = Vec::with_capacity(256);
    for _ in 0..32 {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut mixed = state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d049bb133111eb);
        mixed ^= mixed >> 31;
        result.extend_from_slice(&mixed.to_le_bytes());
    }
    result
}
