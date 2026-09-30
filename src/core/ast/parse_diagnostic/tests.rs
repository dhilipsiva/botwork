use super::*;
use crate::core::{
    ast::{AstFailure, Program, SourceFile},
    diagnostic::{CallFrame, Diagnostic, DiagnosticCode, DiagnosticLimits, SUMMARY_DETAIL_BYTES},
    grammar::BWErr,
};
use std::sync::Arc;

fn owner(text: &str) -> Arc<SourceFile> {
    Arc::new(SourceFile {
        name: "தமிழ்.botwork".into(),
        text: text.into(),
    })
}

#[test]
fn standalone_program_and_native_syntax_errors_admit_exact_default_source_ownership() {
    let text = "Read |🙂|";
    let name = "n".repeat(DiagnosticLimits::default().source_bytes - text.len());
    for native in [false, true] {
        let parse = |name: &str| {
            if native {
                crate::core::ast::native_signature(name, text)
                    .err()
                    .unwrap()
            } else {
                Program::parse_detailed(name, text).unwrap_err()
            }
        };
        let accepted = parse(&name);
        assert_eq!(accepted.code(), DiagnosticCode::Syntax);
        assert_eq!(
            DiagnosticLimits::default()
                .check(&accepted)
                .unwrap()
                .source_bytes,
            DiagnosticLimits::default().source_bytes
        );
        let rejected = parse(&format!("{name}é"));
        assert_eq!(rejected.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(rejected.causes[0].code(), DiagnosticCode::Syntax);
        let omitted = rejected.causes[0].omissions.as_ref().unwrap();
        assert!(omitted.source.as_ref().unwrap().file_truncated);
        assert_eq!(omitted.detail_fields, 1);
        assert!(rejected.causes[0].span.is_none());
    }
}

#[test]
fn streamed_positions_match_pest_for_unicode_tabs_newlines_eof_paths_and_rule_lists() {
    for text in ["", "é\t🙂\r\nnext\n", "\r", "\n\n\n\n\n\n\n\n\n\n\t\tbad"] {
        let source = owner(text);
        for position in (0..=text.len()).filter(|index| text.is_char_boundary(*index)) {
            for positives in 0..=3 {
                for negatives in 0..=3 {
                    let variants = [Rule::ident, Rule::expression, Rule::stmt_block];
                    let mut error = Error::new_from_pos(
                        ErrorVariant::ParsingError {
                            positives: variants[..positives].to_vec(),
                            negatives: variants[..negatives].to_vec(),
                        },
                        pest::Position::new(text, position).unwrap(),
                    );
                    if negatives % 2 == 0 {
                        error = error.with_path("é/தமிழ்.botwork");
                    }
                    let span = Span {
                        source: source.clone(),
                        start: position,
                        end: position,
                    };
                    assert_eq!(
                        ParseDisplay {
                            error: &error,
                            span: &span
                        }
                        .to_string(),
                        error.to_string()
                    );
                }
            }
        }
    }
}

#[test]
fn streamed_spans_match_pest_for_continued_lines_visual_whitespace_and_inverted_columns() {
    for text in [
        "",
        "é\t🙂\r\nnext\n",
        "first\n\nlast",
        "\n\n\n\n\n\n\n\n\n\n\tlast",
    ] {
        let source = owner(text);
        let offsets: Vec<_> = (0..=text.len())
            .filter(|index| text.is_char_boundary(*index))
            .collect();
        for &start in &offsets {
            for &end in offsets.iter().filter(|end| **end >= start) {
                let error = Error::new_from_span(
                    ErrorVariant::CustomError {
                        message: "custom é failure".into(),
                    },
                    pest::Span::new(text, start, end).unwrap(),
                )
                .with_path("é.botwork");
                let span = Span {
                    source: source.clone(),
                    start,
                    end,
                };
                assert_eq!(
                    ParseDisplay {
                        error: &error,
                        span: &span
                    }
                    .to_string(),
                    error.to_string(),
                    "{text:?} {start}..{end}"
                );
            }
        }
    }
}

#[test]
fn syntax_construction_admits_exact_message_source_and_prospective_calls_as_one_diagnostic() {
    let text = "\tbad é input";
    let source = owner(text);
    let parser_error = Error::new_from_pos(
        ErrorVariant::ParsingError {
            positives: vec![Rule::ident],
            negatives: vec![],
        },
        pest::Position::new(text, 5).unwrap(),
    );
    let span = Span {
        source: source.clone(),
        start: 5,
        end: 7,
    };
    let caller = Program::parse("caller", "Read {}").unwrap();
    let frame = CallFrame {
        signature: "read".into(),
        statement: None,
        call_site: caller.statements[0].span.clone(),
        definition_site: None,
    };
    let baseline = Diagnostic::new(BWErr::ParsingError(parser_error.to_string()))
        .at(&span)
        .capture_stack(std::iter::once(&frame));
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    let exact = DiagnosticLimits {
        diagnostics: size.diagnostics,
        depth: size.depth,
        call_frames: size.call_frames,
        related_locations: size.related_locations,
        text_bytes: size.text_bytes,
        source_bytes: size.source_bytes,
    };
    let failure = AstFailure::Syntax {
        error: &parser_error,
        span: &span,
    };
    let accepted = failure.diagnostic(&exact, std::iter::once(&frame));
    assert_eq!(
        accepted.to_value().to_string(),
        baseline.to_value().to_string()
    );
    for dimension in 0..6 {
        let mut limits = exact.clone();
        match dimension {
            0 => limits.diagnostics = 0,
            1 => limits.depth = 0,
            2 => limits.call_frames = 0,
            3 => limits.text_bytes -= 1,
            4 => limits.source_bytes -= 1,
            _ => limits.depth = usize::MAX,
        }
        let rejected = failure.diagnostic(&limits, std::iter::once(&frame));
        assert!(matches!(
            rejected.code(),
            DiagnosticCode::ResourceLimit | DiagnosticCode::RunConfiguration
        ));
        assert_eq!(rejected.causes[0].code(), DiagnosticCode::Syntax);
        let omitted = rejected.causes[0].omissions.as_ref().unwrap();
        assert_eq!(omitted.detail_fields, 1);
        assert_eq!(omitted.call_frames, 1);
        assert!(rejected.causes[0].span.is_none());
    }
}

#[test]
fn rejected_syntax_evidence_skips_source_spans_truncates_utf8_and_releases_source_owners() {
    let source = owner("a\nb");
    let weak = Arc::downgrade(&source);
    let span = Span {
        source: source.clone(),
        start: usize::MAX,
        end: usize::MAX,
    };
    let mut parser_error = Error::new_from_pos(
        ErrorVariant::CustomError {
            message: "🦀".repeat(1024),
        },
        pest::Position::from_start(""),
    );
    // Private sentinel: a full formatter would access this invalid source span.
    parser_error.location = InputLocation::Span((0, 3));
    parser_error.line_col = LineColLocation::Span((1, 1), (2, 1));
    let error = AstFailure::Syntax {
        error: &parser_error,
        span: &span,
    }
    .diagnostic(
        &DiagnosticLimits {
            source_bytes: 0,
            ..DiagnosticLimits::default()
        },
        std::iter::empty(),
    );
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    let BWErr::ParsingError(detail) = error.causes[0].error.as_ref() else {
        panic!("syntax")
    };
    assert!(detail.contains("source excerpt omitted") && detail.ends_with("…[truncated]"));
    assert!(detail.len() <= SUMMARY_DETAIL_BYTES);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    drop(span);
    drop(source);
    assert!(weak.upgrade().is_none());
}
