use super::*;
use crate::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, SUMMARY_DETAIL_BYTES, SUMMARY_SOURCE_NAME_BYTES},
};

#[test]
fn cause_construction_rejects_complete_primary_context_before_visiting_the_cause_formatter() {
    struct Forbidden;
    impl std::fmt::Display for Forbidden {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("cause formatter visited")
        }
    }
    let program = Program::parse("source", "Read {}").unwrap();
    let weak = Arc::downgrade(&program.source);
    for limits in [
        DiagnosticLimits {
            diagnostics: 1,
            ..DiagnosticLimits::default()
        },
        DiagnosticLimits {
            depth: 1,
            ..DiagnosticLimits::default()
        },
        DiagnosticLimits {
            text_bytes: "source".len() * 2,
            ..DiagnosticLimits::default()
        },
        DiagnosticLimits {
            source_bytes: 0,
            ..DiagnosticLimits::default()
        },
    ] {
        let error = limits
            .formatted_cause(
                Diagnostic::new(BWErr::Cancelled("stopped".into())),
                BWErr::AsyncRuntime,
                format_args!("{}", Forbidden),
                &program.statements[0].span,
            )
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Cancelled);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().direct_causes, 1);
        assert!(error.causes[0].span.is_none());
    }
    drop(program);
    assert!(weak.upgrade().is_none());
}

#[test]
fn source_prefix_admits_complete_call_context_and_distinct_equal_source_owners_before_copying() {
    let caller = Program::parse("file", "Read {}").unwrap();
    let frame = CallFrame {
        signature: "read".into(),
        call_site: caller.statements[0].span.clone(),
        definition_site: None,
    };
    let original = || BWErr::ResourceLimit {
        resource: "source bytes",
        limit: 4,
    };
    let baseline = Diagnostic::new(original())
        .at(&Span::source_prefix("file", "Read {}", 4))
        .capture_stack(std::iter::once(&frame));
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    assert_eq!(size.source_bytes, 2 * ("file".len() + "Read {}".len()));
    let exact = DiagnosticLimits {
        diagnostics: size.diagnostics,
        depth: size.depth,
        call_frames: size.call_frames,
        related_locations: size.related_locations,
        text_bytes: size.text_bytes,
        source_bytes: size.source_bytes,
    };
    let error = exact.source_prefix(original(), "file", "Read {}", 4, std::iter::once(&frame));
    assert_eq!(
        error.to_value().to_string(),
        baseline.to_value().to_string()
    );
    assert!(!Arc::ptr_eq(
        error.span.as_ref().unwrap().source(),
        &caller.source
    ));
    for dimension in 0..5 {
        let mut limits = exact.clone();
        match dimension {
            0 => limits.source_bytes -= 1,
            1 => limits.text_bytes -= 1,
            2 => limits.call_frames = 0,
            3 => limits.depth = 0,
            _ => limits.diagnostics = 0,
        }
        let error = limits.source_prefix(original(), "file", "Read {}", 4, std::iter::once(&frame));
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].error.to_string(), original().to_string());
        assert!(error.causes[0].span.is_none());
        let omitted = error.causes[0].omissions.as_ref().unwrap();
        assert_eq!(omitted.call_frames, 1);
        assert_eq!(omitted.detail_fields, 0);
        let source = omitted.source.as_ref().unwrap();
        assert_eq!((source.start_byte, source.end_byte), (4, 7));
    }
}

#[test]
fn rejected_source_prefix_preserves_large_offsets_unicode_and_configuration_without_source_owners()
{
    let name = "🦀".repeat(1024);
    let error = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .source_prefix(
        BWErr::RunConfiguration("invalid syntax configuration".into()),
        &name,
        "",
        usize::MAX,
        std::iter::empty(),
    );
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::RunConfiguration);
    assert!(error.causes[0].span.is_none());
    let source = error.causes[0]
        .omissions
        .as_ref()
        .unwrap()
        .source
        .as_ref()
        .unwrap();
    assert!(source.file_truncated && source.file.ends_with("…[truncated]"));
    assert!(source.file.len() <= SUMMARY_SOURCE_NAME_BYTES);
    assert_eq!((source.start_byte, source.end_byte), (usize::MAX, 0));
}

#[test]
fn alternative_detail_summaries_skip_full_formatters_after_context_rejection() {
    struct Forbidden;
    impl std::fmt::Display for Forbidden {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("rejected location formatter was visited")
        }
    }
    let program = Program::parse("source", "Read {}").unwrap();
    let span = &program.statements[0].span;
    let owner = Arc::downgrade(&program.source);
    let error = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .formatted_related_fields(
        |[signature, original, duplicate]| BWErr::DuplicateStatement {
            signature,
            original,
            duplicate,
        },
        [
            FormattedDetail {
                full: format_args!("read"),
                summary: None,
            },
            FormattedDetail {
                full: format_args!("{}", Forbidden),
                summary: Some(format_args!("file:[byte 0; coordinates omitted]")),
            },
            FormattedDetail {
                full: format_args!("{}", Forbidden),
                summary: Some(format_args!("file:[byte 4; coordinates omitted]")),
            },
        ],
        span,
        ("first definition", span),
        std::iter::empty(),
    );
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::DuplicateStatement);
    let omitted = error.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omitted.detail_fields, 2);
    assert_eq!(omitted.related_locations, 1);
    assert!(error.causes[0].span.is_none());
    drop(program);
    assert!(owner.upgrade().is_none());
}

#[test]
fn alternative_fields_count_precision_loss_and_prefix_truncation_once_per_field() {
    use std::cell::Cell;
    struct Full<'a>(&'a Cell<usize>);
    impl std::fmt::Display for Full<'_> {
        fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.set(self.0.get() + 1);
            out.write_str("source:1:1")
        }
    }
    let visits = Cell::new(0);
    let program = Program::parse("source", "Read {}").unwrap();
    let span = &program.statements[0].span;
    let long = "🦀".repeat(1024);
    let make = |text_bytes| {
        DiagnosticLimits {
            text_bytes,
            ..DiagnosticLimits::default()
        }
        .formatted_related_fields(
            |[signature, original, duplicate]| BWErr::DuplicateStatement {
                signature,
                original,
                duplicate,
            },
            [
                FormattedDetail {
                    full: format_args!("read"),
                    summary: None,
                },
                FormattedDetail {
                    full: format_args!("{}", Full(&visits)),
                    summary: Some(format_args!("{long}")),
                },
                FormattedDetail {
                    full: format_args!("{}", Full(&visits)),
                    summary: Some(format_args!("source:[byte 0; coordinates omitted]")),
                },
            ],
            span,
            ("first definition", span),
            std::iter::empty(),
        )
    };
    let exact = "source".len() + "first definition".len() + "read".len() + 2 * "source:1:1".len();
    let accepted = make(exact);
    assert_eq!(accepted.code(), DiagnosticCode::DuplicateStatement);
    assert!(accepted.omissions.is_none());
    assert_eq!(visits.get(), 4); // Two measurements and two admitted copies.
    visits.set(0);
    let error = make(exact - 1);
    assert_eq!(visits.get(), 2); // Measurement only; rejection uses alternatives.
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 2);
    let BWErr::DuplicateStatement {
        original,
        duplicate,
        ..
    } = error.causes[0].error.as_ref()
    else {
        panic!("duplicate")
    };
    assert!(original.len() <= 256 && original.ends_with("…[truncated]"));
    assert_eq!(duplicate, "source:[byte 0; coordinates omitted]");
}

#[test]
fn formatted_related_details_admit_complete_site_and_call_metrics_before_message_copying() {
    let primary = Program::parse("primary", "|x| = |1|").unwrap();
    let related = Program::parse("related-é", "|y| = |2|").unwrap();
    let span = &primary.statements[0].span;
    let site = &related.statements[0].span;
    let frames = [CallFrame {
        signature: "outer".into(),
        call_site: span.clone(),
        definition_site: None,
    }];
    let baseline = Diagnostic::new(BWErr::ImportRead("reason".into()))
        .at(span)
        .with_related("imported here", site)
        .capture_stack(frames.iter());
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    let exact = DiagnosticLimits {
        diagnostics: size.diagnostics,
        depth: size.depth,
        call_frames: size.call_frames,
        related_locations: size.related_locations,
        text_bytes: size.text_bytes,
        source_bytes: size.source_bytes,
    };
    let construct = |limits: &DiagnosticLimits| {
        limits.formatted_related_detail(
            BWErr::ImportRead,
            format_args!("reason"),
            span,
            ("imported here", site),
            frames.iter(),
        )
    };
    assert_eq!(
        construct(&exact).to_value().to_string(),
        baseline.to_value().to_string()
    );
    for limits in [
        DiagnosticLimits {
            related_locations: 0,
            ..exact.clone()
        },
        DiagnosticLimits {
            text_bytes: size.text_bytes - 1,
            ..exact.clone()
        },
        DiagnosticLimits {
            source_bytes: size.source_bytes - 1,
            ..exact.clone()
        },
        DiagnosticLimits {
            call_frames: 0,
            ..exact.clone()
        },
    ] {
        let error = construct(&limits);
        assert!(error.is_emergency());
        assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
        let omitted = error.causes[0].omissions.as_ref().unwrap();
        assert_eq!(omitted.related_locations, 1);
        assert_eq!(omitted.call_frames, 1);
        assert_eq!(omitted.source.as_ref().unwrap().file, "primary");
        assert!(error.causes[0].span.is_none() && error.causes[0].related.is_empty());
    }
}

#[test]
fn streamed_input_origins_stop_at_the_source_allowance_and_bound_rejection_evidence() {
    use std::cell::Cell;
    struct Origin<'a>(&'a Cell<usize>);
    impl fmt::Display for Origin<'_> {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..1024 {
                self.0.set(self.0.get() + 1);
                output.write_str("é")?;
            }
            Ok(())
        }
    }
    for (source_bytes, diagnostics, expected_visits) in
        [(2048, 1, 2048), (0, 1, 130), (2048, 0, 129)]
    {
        let visits = Cell::new(0);
        let error = DiagnosticLimits {
            source_bytes,
            diagnostics,
            ..DiagnosticLimits::default()
        }
        .input_origin(
            BWErr::ResourceLimit {
                resource: "input sources",
                limit: 0,
            },
            &Origin(&visits),
        );
        assert_eq!(visits.get(), expected_visits);
        if source_bytes == 2048 && diagnostics == 1 {
            assert_eq!(
                error.span.as_ref().unwrap().source().name(),
                "é".repeat(1024)
            );
            assert!(error.causes.is_empty());
        } else {
            let evidence = error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap();
            assert!(evidence.file_truncated && evidence.file.len() <= SUMMARY_SOURCE_NAME_BYTES);
            assert!(error.causes[0].span.is_none());
            assert_eq!((evidence.start_byte, evidence.end_byte), (0, 0));
        }
    }
}

#[cfg(unix)]
#[test]
fn streamed_native_input_origins_count_lossy_display_bytes_at_the_exact_boundary() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};
    let path = PathBuf::from(OsString::from_vec(vec![b'x', 0xff, 0xfe, b'.', b'j']));
    let display = path.display().to_string();
    assert_eq!(display, "x��.j");
    for rejected in [false, true] {
        let error = DiagnosticLimits {
            source_bytes: display.len() - usize::from(rejected),
            ..DiagnosticLimits::default()
        }
        .input_origin(
            BWErr::ResourceLimit {
                resource: "input sources",
                limit: 0,
            },
            &path.display(),
        );
        if rejected {
            let evidence = error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap();
            assert_eq!(evidence.file, display);
            assert!(!evidence.file_truncated);
            assert!(error.causes[0].span.is_none());
        } else {
            assert_eq!(error.span.as_ref().unwrap().source().name(), display);
            assert!(error.causes.is_empty());
        }
    }
}

#[test]
fn input_origin_construction_measures_the_source_name_before_retaining_it() {
    let origin = "config-é.json";
    let resource = || BWErr::ResourceLimit {
        resource: "input source bytes",
        limit: 0,
    };
    let baseline = Diagnostic::new(resource()).at(&Span::input_origin(origin));
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    let exact = DiagnosticLimits {
        diagnostics: 1,
        depth: 1,
        call_frames: 0,
        related_locations: 0,
        text_bytes: size.text_bytes,
        source_bytes: origin.len(),
    };
    let error = exact.input_origin(resource(), origin);
    assert_eq!(
        error.to_value().to_string(),
        baseline.to_value().to_string()
    );
    assert_eq!(exact.check(&error).unwrap(), size);
    assert_eq!(error.span.as_ref().unwrap().line_column(), (1, 1));
    assert_eq!(error.span.as_ref().unwrap().source().text(), "");
    assert!(error.causes.is_empty() && error.omissions.is_none());
    for limits in [
        DiagnosticLimits {
            source_bytes: origin.len() - 1,
            ..exact.clone()
        },
        DiagnosticLimits {
            text_bytes: size.text_bytes - 1,
            ..exact.clone()
        },
        DiagnosticLimits {
            diagnostics: 0,
            ..exact.clone()
        },
        DiagnosticLimits {
            depth: 0,
            ..exact.clone()
        },
        DiagnosticLimits {
            depth: usize::MAX,
            ..exact.clone()
        },
    ] {
        let error = limits.input_origin(resource(), origin);
        assert!(error.is_emergency());
        assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
        assert!(matches!(
            error.causes[0].error.as_ref(),
            BWErr::ResourceLimit {
                resource: "input source bytes",
                limit: 0
            }
        ));
        assert!(error.span.is_none() && error.causes[0].span.is_none());
        let evidence = error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap();
        assert_eq!(evidence.file, origin);
        assert!(!evidence.file_truncated);
        assert_eq!((evidence.start_byte, evidence.end_byte), (0, 0));
    }
    let error = DiagnosticLimits {
        source_bytes: 0,
        ..exact
    }
    .input_origin(resource(), "");
    assert!(error.causes.is_empty());
    assert_eq!(error.span.as_ref().unwrap().source().name(), "");
}

#[test]
fn rejected_input_origins_have_utf8_bounded_evidence_without_source_owners() {
    let origin = "🦀".repeat(1024);
    let error = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .input_origin(
        BWErr::ResourceLimit {
            resource: "input sources",
            limit: 0,
        },
        &origin,
    );
    let cause = &error.causes[0];
    assert!(error.span.is_none() && cause.span.is_none());
    let omitted = cause.omissions.as_ref().unwrap();
    assert_eq!(omitted.detail_fields, 0);
    let source = omitted.source.as_ref().unwrap();
    assert!(source.file_truncated && source.file.len() <= SUMMARY_SOURCE_NAME_BYTES);
    assert!(source.file.ends_with("…[truncated]"));
    assert_eq!((source.start_byte, source.end_byte), (0, 0));
    assert_eq!(
        DiagnosticLimits::default()
            .check(&error)
            .unwrap()
            .source_bytes,
        0
    );
}

#[test]
fn exact_borrowed_details_match_full_diagnostics_and_preserve_their_call_order() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let frames = [
        CallFrame {
            signature: "outer".into(),
            call_site: span.clone(),
            definition_site: None,
        },
        CallFrame {
            signature: "inner".into(),
            call_site: span.clone(),
            definition_site: Some(span.clone()),
        },
    ];
    for category in [
        BWErr::VariableNotDefined,
        BWErr::StatementNotDefined,
        BWErr::NativePanic,
    ] {
        for expression in [false, true] {
            let full = Diagnostic::new(category("é".into()));
            let full = if expression {
                full.at_expression(span)
            } else {
                full.at(span)
            }
            .capture_stack(frames.iter());
            let size = DiagnosticLimits::default().check(&full).unwrap();
            let limits = DiagnosticLimits {
                diagnostics: size.diagnostics,
                depth: size.depth,
                call_frames: size.call_frames,
                related_locations: 0,
                text_bytes: size.text_bytes,
                source_bytes: size.source_bytes,
            };
            let built =
                limits.borrowed_detail(category, "é", Some(span), expression, frames.iter());
            assert_eq!(built.code(), full.code());
            assert_eq!(built.to_value().to_string(), full.to_value().to_string());
            assert_eq!(limits.check(&built).unwrap(), size);
            assert_eq!(built.call_stack[0].signature, "inner");
        }
    }
}

#[test]
fn every_prospective_quota_rejects_before_context_capture_with_exact_omission_counts() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let frames = [CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: None,
    }];
    for field in 0..5 {
        let mut limits = DiagnosticLimits::default();
        match field {
            0 => limits.diagnostics = 0,
            1 => limits.depth = 0,
            2 => limits.call_frames = 0,
            3 => limits.text_bytes = "source".len() + "read".len(),
            _ => limits.source_bytes = 0,
        }
        let error = limits.borrowed_detail(
            BWErr::NativePanic,
            "panic",
            Some(span),
            false,
            frames.iter(),
        );
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert!(error.is_emergency());
        let summary = &error.causes[0];
        assert_eq!(summary.code(), DiagnosticCode::NativePanic);
        let omitted = summary.omissions.as_ref().unwrap();
        assert_eq!(omitted.detail_fields, 0);
        assert_eq!(omitted.call_frames, 1);
        assert_eq!(omitted.source.as_ref().unwrap().start_byte, 0);
        assert!(summary.span.is_none());
    }
}

#[test]
fn rejected_large_unicode_details_and_source_names_have_explicit_bounded_evidence() {
    let program = Program::parse(&"é".repeat(4096), "|x| = |1|").unwrap();
    let source = Arc::downgrade(&program.source);
    let error = DiagnosticLimits {
        text_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .borrowed_detail(
        BWErr::VariableNotDefined,
        &"🦀".repeat(4096),
        Some(&program.statements[0].span),
        true,
        std::iter::empty(),
    );
    drop(program);
    assert!(source.upgrade().is_none());
    let summary = &error.causes[0];
    let BWErr::VariableNotDefined(detail) = summary.error.as_ref() else {
        panic!("category")
    };
    assert!(detail.len() <= SUMMARY_DETAIL_BYTES && detail.ends_with("…[truncated]"));
    let omitted = summary.omissions.as_ref().unwrap();
    assert_eq!(omitted.detail_fields, 1);
    assert_eq!(summary.label, "expression");
    assert!(omitted.source.as_ref().unwrap().file.len() <= SUMMARY_SOURCE_NAME_BYTES);
    assert!(omitted.source.as_ref().unwrap().file_truncated);
}

#[test]
fn empty_details_and_invalid_configuration_preserve_category_without_source_owners() {
    let exact = DiagnosticLimits {
        diagnostics: 1,
        depth: 1,
        call_frames: 0,
        related_locations: 0,
        text_bytes: 6,
        source_bytes: 0,
    };
    assert_eq!(
        exact
            .borrowed_detail(BWErr::NativePanic, "", None, false, std::iter::empty())
            .code(),
        DiagnosticCode::NativePanic
    );
    let error = DiagnosticLimits { depth: 65, ..exact }.borrowed_detail(
        BWErr::NativePanic,
        "panic",
        None,
        false,
        std::iter::empty(),
    );
    assert_eq!(error.code(), DiagnosticCode::RunConfiguration);
    assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
    assert!(error.causes[0].omissions.as_ref().unwrap().source.is_none());
}

#[test]
fn formatted_details_match_exact_raw_byte_counts_and_preserve_context() {
    let program = Program::parse("é", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let frames = [CallFrame {
        signature: "read".into(),
        call_site: span.clone(),
        definition_site: None,
    }];
    let message = "Parameter `é` needs Int | Float; got Bool (argument 12)";
    let baseline = Diagnostic::new(BWErr::OperationIncompatibleError(message.into()))
        .at_expression(span)
        .capture_stack(frames.iter());
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    let kinds = crate::core::signature::ValueKinds::one(crate::core::signature::ValueKind::Int)
        .union(crate::core::signature::ValueKinds::one(
            crate::core::signature::ValueKind::Float,
        ));
    for fits in [false, true] {
        let limits = DiagnosticLimits {
            text_bytes: size.text_bytes - usize::from(!fits),
            source_bytes: size.source_bytes,
            ..DiagnosticLimits::default()
        };
        let error = limits.formatted_detail(
            BWErr::OperationIncompatibleError,
            format_args!(
                "Parameter `{}` needs {kinds}; got {} (argument {})",
                "é", "Bool", 12
            ),
            Some(span),
            true,
            frames.iter(),
        );
        if fits {
            assert_eq!(
                error.to_value().to_string(),
                baseline.to_value().to_string()
            );
        } else {
            assert!(error.is_emergency());
            assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 0);
        }
    }
}

#[test]
fn formatting_writers_enforce_overflow_and_unicode_boundaries_without_partial_count_changes() {
    let mut count = Counter {
        bytes: usize::MAX,
        maximum: usize::MAX,
    };
    assert!(count.write_str("x").is_err());
    assert_eq!(count.bytes, usize::MAX);
    let mut count = Counter {
        bytes: 2,
        maximum: 3,
    };
    assert!(count.write_str("é").is_err());
    assert_eq!(count.bytes, 2);
    count.write_str("x").unwrap();
    assert_eq!(count.bytes, 3);
    let mut output = BoundedText {
        text: String::new(),
        maximum: 3,
    };
    assert!(output.write_str("é🦀").is_err());
    assert_eq!(output.text, "é");
    output.write_str("x").unwrap();
    assert_eq!(output.text, "éx");
}

#[test]
fn formatted_prefixes_mark_truncation_once_and_stop_visiting_later_fragments() {
    use std::cell::Cell;
    struct Fragments<'a>(&'a Cell<usize>);
    impl fmt::Display for Fragments<'_> {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..1000 {
                self.0.set(self.0.get() + 1);
                formatter.write_str("🦀")?;
            }
            Ok(())
        }
    }
    let visits = Cell::new(0);
    let error = DiagnosticLimits {
        text_bytes: 7,
        ..DiagnosticLimits::default()
    }
    .formatted_detail(
        BWErr::OperationIncompatibleError,
        format_args!("{}", Fragments(&visits)),
        None,
        false,
        std::iter::empty(),
    );
    assert_eq!(visits.get(), 66); // one count attempt, then 64 complete prefix chars plus overflow
    let summary = &error.causes[0];
    assert_eq!(summary.omissions.as_ref().unwrap().detail_fields, 1);
    let BWErr::OperationIncompatibleError(detail) = summary.error.as_ref() else {
        panic!("category")
    };
    assert!(detail.len() <= SUMMARY_DETAIL_BYTES && detail.ends_with("…[truncated]"));
    let exact = "é".repeat(128);
    assert_eq!(
        formatted_prefix(format_args!("{exact}")),
        (exact.clone(), false)
    );
    let (shortened, truncated) = formatted_prefix(format_args!("{exact}x"));
    assert!(truncated && shortened.len() <= SUMMARY_DETAIL_BYTES);
}

#[test]
fn formatted_construction_rejects_context_before_full_message_formatting() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let error = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .formatted_detail(
        BWErr::OperationIncompatibleError,
        format_args!("{}", "é".repeat(2048)),
        Some(&program.statements[0].span),
        false,
        std::iter::empty(),
    );
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(error.is_emergency());
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    let accepted = DiagnosticLimits {
        text_bytes: 6,
        source_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .formatted_detail(
        BWErr::OperationIncompatibleError,
        format_args!(""),
        None,
        false,
        std::iter::empty(),
    );
    assert_eq!(accepted.code(), DiagnosticCode::IncompatibleType);
}

#[test]
fn grouped_details_match_complete_metrics_before_any_field_is_owned() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let span = &program.statements[0].span;
    let category = |[path, segment, reason]: [String; 3]| BWErr::CollectionAccessError {
        path,
        segment,
        reason,
    };
    let baseline = Diagnostic::new(category(["data.é".into(), "é".into(), "length 12".into()]))
        .at_expression(span);
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    for fits in [false, true] {
        let limits = DiagnosticLimits {
            text_bytes: size.text_bytes - usize::from(!fits),
            source_bytes: size.source_bytes,
            ..DiagnosticLimits::default()
        };
        let error = limits.formatted_fields(
            category,
            [
                format_args!("data.{}", "é"),
                format_args!("é"),
                format_args!("length {}", 12),
            ],
            Some(span),
            true,
            std::iter::empty(),
        );
        if fits {
            assert_eq!(
                error.to_value().to_string(),
                baseline.to_value().to_string()
            );
        } else {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::CollectionAccess);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 0);
        }
    }
}

#[test]
fn rejection_in_a_later_field_never_runs_owned_formatting_of_an_earlier_field() {
    use std::cell::Cell;
    struct First<'a>(&'a Cell<usize>);
    impl fmt::Display for First<'_> {
        fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.set(self.0.get() + 1);
            output.write_str("first")
        }
    }
    let visits = Cell::new(0);
    let error = DiagnosticLimits {
        text_bytes: 17,
        ..DiagnosticLimits::default()
    }
    .formatted_fields(
        |[path, segment, reason]| BWErr::CollectionAccessError {
            path,
            segment,
            reason,
        },
        [
            format_args!("{}", First(&visits)),
            format_args!("second"),
            format_args!("reason"),
        ],
        None,
        false,
        std::iter::empty(),
    );
    assert!(error.is_emergency());
    assert_eq!(visits.get(), 2); // measure plus emergency prefix; no admitted message construction
}

#[test]
fn grouped_rejection_records_each_shortened_unicode_field_once_and_releases_sources() {
    let program = Program::parse("source", "|x| = |1|").unwrap();
    let source = Arc::downgrade(&program.source);
    let long = "🦀".repeat(1024);
    let error = DiagnosticLimits {
        source_bytes: 0,
        ..DiagnosticLimits::default()
    }
    .formatted_fields(
        |[path, segment, reason]| BWErr::CollectionAccessError {
            path,
            segment,
            reason,
        },
        [
            format_args!("{long}"),
            format_args!("{long}"),
            format_args!("{long}"),
        ],
        Some(&program.statements[0].span),
        true,
        std::iter::empty(),
    );
    drop(program);
    assert!(source.upgrade().is_none());
    let summary = &error.causes[0];
    assert_eq!(summary.omissions.as_ref().unwrap().detail_fields, 3);
    let BWErr::CollectionAccessError {
        path,
        segment,
        reason,
    } = summary.error.as_ref()
    else {
        panic!("category")
    };
    for detail in [path, segment, reason] {
        assert!(detail.len() <= SUMMARY_DETAIL_BYTES && detail.ends_with("…[truncated]"));
    }
}
