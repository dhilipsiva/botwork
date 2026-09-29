use super::*;
use crate::core::{
    ast::Program,
    diagnostic::{CallFrame, DiagnosticOmissions, OmittedSource, RelatedLocation},
    signature::{StatementSignature, ValueKind},
};
use std::{collections::HashMap, sync::Arc};

fn response(body: &[u8]) -> Vec<u8> {
    let mut frame = b"BWIP\x01\x00\x01".to_vec();
    frame.extend_from_slice(&(body.len() as u64).to_le_bytes());
    frame.extend_from_slice(body);
    frame
}
fn error() -> Diagnostic {
    let program = Program::parse("worker-é", "|x| = |\"é\"|").unwrap();
    let span = &program.statements[0].span;
    let mut error = Diagnostic::new(BWErr::NativeError("failed é".into())).at(span);
    error.call_stack.push(CallFrame {
        signature: "native".into(),
        call_site: span.clone(),
        definition_site: Some(span.clone()),
    });
    error.related.push(RelatedLocation {
        message: "related".into(),
        span: span.clone(),
    });
    error.causes.push(Diagnostic {
        error: error.error.clone(),
        ..Diagnostic::new(BWErr::OutputError("unused".into())).at(span)
    });
    error.omissions = Some(Box::new(DiagnosticOmissions {
        detail_fields: 1,
        call_frames: 2,
        related_locations: 3,
        direct_causes: 4,
        label: true,
        prior_summary: true,
        source: Some(OmittedSource {
            file: "prior".into(),
            file_truncated: true,
            start_byte: 2,
            end_byte: 9,
        }),
    }));
    error
}

#[test]
fn typed_values_round_trip_exact_float_bits_and_canonical_maps() {
    let protocol = WorkerProtocol::default();
    let values = vec![
        Literal::None,
        Literal::Int(i32::MIN),
        Literal::Int(i32::MAX),
        Literal::Bool(false),
        Literal::Bool(true),
        Literal::Float(-0.0),
        Literal::Float(f32::MIN_POSITIVE),
        Literal::Float(f32::MAX),
        Literal::String("é\0\n".into()),
        Literal::Array(vec![Literal::None]),
        Literal::Map(HashMap::from([
            ("é".into(), Literal::Int(1)),
            ("a".into(), Literal::Float(1.0)),
        ])),
    ];
    let frame = protocol.encode_request(&values).unwrap();
    let decoded = protocol.decode_request(&frame).unwrap();
    assert_eq!(protocol.encode_request(&decoded).unwrap(), frame);
    for value in values {
        let frame = protocol.encode_response(Ok(&value)).unwrap();
        let result = protocol.decode_response(&frame).unwrap().unwrap();
        assert_eq!(result.kind(), value.kind());
        assert_eq!(protocol.encode_response(Ok(&result)).unwrap(), frame);
    }
    assert_eq!(
        protocol.encode_response(Ok(&Literal::Float(-0.0))).unwrap(),
        response(&[2, 0, 0, 0, 128])
    );
}

#[test]
fn malformed_headers_truncation_and_trailing_bytes_are_rejected() {
    let protocol = WorkerProtocol::default();
    let frame = protocol
        .encode_response(Ok(&Literal::String("hello".into())))
        .unwrap();
    for length in 0..frame.len() {
        assert!(
            protocol.decode_response(&frame[..length]).is_err(),
            "{length}"
        );
    }
    for (index, byte) in [(0, 0), (4, 2), (6, 0), (7, 255)] {
        let mut invalid = frame.clone();
        invalid[index] = byte;
        assert!(protocol.decode_response(&invalid).is_err());
    }
    let mut invalid = frame;
    invalid.push(0);
    assert!(protocol.decode_response(&invalid).is_err());
}

#[test]
fn values_reject_unknown_tags_nonfinite_numbers_bad_booleans_and_utf8() {
    let protocol = WorkerProtocol::default();
    for body in [
        vec![7],
        vec![3, 2],
        [vec![2], f32::NAN.to_bits().to_le_bytes().to_vec()].concat(),
        [vec![2], f32::INFINITY.to_bits().to_le_bytes().to_vec()].concat(),
        [vec![4], 1u64.to_le_bytes().to_vec(), vec![255]].concat(),
        [vec![5], u64::MAX.to_le_bytes().to_vec()].concat(),
    ] {
        assert!(
            protocol.decode_response(&response(&body)).is_err(),
            "{body:?}"
        );
    }
    for value in [
        Literal::Float(f32::NAN),
        Literal::Array(vec![Literal::Float(f32::NEG_INFINITY)]),
    ] {
        assert!(protocol
            .encode_request(std::slice::from_ref(&value))
            .is_err());
        assert!(protocol.encode_response(Ok(&value)).is_err());
    }
}

#[test]
fn map_keys_must_be_sorted_and_unique() {
    for keys in [["a", "a"], ["z", "a"]] {
        let mut body = vec![6];
        body.extend_from_slice(&2u64.to_le_bytes());
        for key in keys {
            body.extend_from_slice(&1u64.to_le_bytes());
            body.extend_from_slice(key.as_bytes());
            body.push(0);
        }
        assert!(WorkerProtocol::default()
            .decode_response(&response(&body))
            .is_err());
    }
}

#[test]
fn exact_and_exceeded_frame_and_argument_budgets() {
    let mut protocol = WorkerProtocol::default();
    let values = [Literal::String("é".into()), Literal::Int(1)];
    let frame = protocol.encode_request(&values).unwrap();
    protocol.limits.frame_bytes = frame.len();
    protocol.limits.arguments = 2;
    protocol.limits.argument_nodes = 2;
    protocol.limits.argument_payload_bytes = 6;
    assert_eq!(
        protocol
            .encode_request(&protocol.decode_request(&frame).unwrap())
            .unwrap(),
        frame
    );
    for dimension in 0..4 {
        let mut limited = protocol.clone();
        match dimension {
            0 => limited.limits.frame_bytes -= 1,
            1 => limited.limits.arguments -= 1,
            2 => limited.limits.argument_nodes -= 1,
            _ => limited.limits.argument_payload_bytes -= 1,
        }
        assert!(limited.encode_request(&values).is_err());
        assert!(limited.decode_request(&frame).is_err());
    }
    protocol.limits.frame_bytes = 0;
    assert!(protocol.encode_request(&[]).is_err());
    assert!(protocol.read_request(&mut &frame[..]).is_err());
}

#[test]
fn per_value_budgets_and_depth_are_checked_on_wire() {
    let protocol = WorkerProtocol::default();
    for (dimension, value) in [
        (0, Literal::Array(vec![Literal::None])),
        (1, Literal::Array(vec![Literal::None])),
        (2, Literal::String("a".into())),
        (
            3,
            Literal::Map(HashMap::from([("a".into(), Literal::None)])),
        ),
        (4, Literal::Array(vec![Literal::None])),
        (5, Literal::Int(1)),
    ] {
        let frame = protocol.encode_response(Ok(&value)).unwrap();
        let mut limited = protocol.clone();
        match dimension {
            0 => limited.limits.values.nodes = 1,
            1 => limited.limits.values.depth = 1,
            2 => limited.limits.values.string_bytes = 0,
            3 => limited.limits.values.key_bytes = 0,
            4 => limited.limits.values.entries = 0,
            _ => limited.limits.values.payload_bytes = 3,
        }
        assert!(limited.decode_response(&frame).is_err());
    }
    let mut body = Vec::new();
    for _ in 0..1000 {
        body.push(5);
        body.extend_from_slice(&1u64.to_le_bytes());
    }
    body.push(0);
    assert!(protocol.decode_response(&response(&body)).is_err());
}

#[test]
fn all_diagnostic_categories_preserve_exact_fields() {
    let mut errors = vec![
        BWErr::DuplicateParameter {
            name: "é".into(),
            original: "one".into(),
            duplicate: "two".into(),
        },
        BWErr::DuplicateStatement {
            signature: "a".into(),
            original: "b".into(),
            duplicate: "c".into(),
        },
        BWErr::CollectionAccessError {
            path: "a".into(),
            segment: "b".into(),
            reason: "c".into(),
        },
        BWErr::DuplicateNamespace {
            namespace: "a".into(),
            original: "b".into(),
            duplicate: "c".into(),
        },
        BWErr::ResourceLimit {
            resource: "value nodes",
            limit: u64::MAX,
        },
        BWErr::ConditionNotMet {
            reason: "é".into(),
            attempts: "1".into(),
            history: "[\0]".into(),
        },
        BWErr::RetriesExhausted {
            reason: "a".into(),
            attempts: "b".into(),
            history: "c".into(),
        },
    ];
    let categories: [fn(String) -> BWErr; 22] = [
        BWErr::ParsingError,
        BWErr::ControlFlowError,
        BWErr::SignatureError,
        BWErr::VariableNotDefined,
        BWErr::StatementNotDefined,
        BWErr::ParameterMissingError,
        BWErr::ParsingIntegerError,
        BWErr::ArithmeticError,
        BWErr::OperationIncompatibleError,
        BWErr::OutputError,
        BWErr::NativeError,
        BWErr::NativePanic,
        BWErr::Cancelled,
        BWErr::Timeout,
        BWErr::AsyncRuntime,
        BWErr::ImportRead,
        BWErr::ImportCycle,
        BWErr::InputError,
        BWErr::RunConfiguration,
        BWErr::SourceRead,
        BWErr::AssertionFailed,
        BWErr::ExplicitFailure,
    ];
    errors.extend(categories.map(|category| category("detail é\0".into())));
    let protocol = WorkerProtocol::default();
    for error in errors {
        let error = Diagnostic::new(error);
        let frame = protocol.encode_response(Err(&error)).unwrap();
        let result = protocol.decode_response(&frame).unwrap().unwrap_err();
        assert_eq!(result.code(), error.code());
        assert_eq!(format!("{:?}", result.error), format!("{:?}", error.error));
        assert_eq!(protocol.encode_response(Err(&result)).unwrap(), frame);
    }
}

#[test]
fn diagnostic_sources_errors_context_and_omissions_keep_shared_identity() {
    let protocol = WorkerProtocol::default();
    let original = error();
    let frame = protocol.encode_response(Err(&original)).unwrap();
    let error = protocol.decode_response(&frame).unwrap().unwrap_err();
    assert_eq!(protocol.encode_response(Err(&error)).unwrap(), frame);
    assert_eq!(
        protocol.limits.diagnostics.check(&error).unwrap(),
        protocol.limits.diagnostics.check(&original).unwrap()
    );
    assert!(Arc::ptr_eq(&error.error, &error.causes[0].error));
    let source = error.span.as_ref().unwrap().source();
    assert!(Arc::ptr_eq(source, error.call_stack[0].call_site.source()));
    assert!(Arc::ptr_eq(source, error.related[0].span.source()));
    assert!(Arc::ptr_eq(
        source,
        error.causes[0].span.as_ref().unwrap().source()
    ));
    assert!(!Arc::ptr_eq(
        source,
        original.span.as_ref().unwrap().source()
    ));
}

#[test]
fn static_names_require_explicit_catalog_entries() {
    let protocol = WorkerProtocol::default();
    let mut error = Diagnostic::new(BWErr::ResourceLimit {
        resource: "host widgets",
        limit: 3,
    });
    error.label = "widget";
    assert!(protocol.encode_response(Err(&error)).is_err());
    let custom = WorkerProtocol {
        resource_names: &["host widgets"],
        labels: &["widget"],
        ..Default::default()
    };
    let frame = custom.encode_response(Err(&error)).unwrap();
    assert!(protocol.decode_response(&frame).is_err());
    assert_eq!(
        custom.decode_response(&frame).unwrap().unwrap_err().label,
        "widget"
    );
}

#[test]
fn diagnostic_limits_apply_exactly_and_rejected_summaries_remain_bounded() {
    let mut original = error();
    original.error = Arc::new(BWErr::NativeError("é".repeat(1000)));
    let mut protocol = WorkerProtocol::default();
    let size = protocol.limits.diagnostics.check(&original).unwrap();
    protocol.limits.diagnostics = DiagnosticLimits {
        diagnostics: size.diagnostics,
        depth: size.depth,
        call_frames: size.call_frames,
        related_locations: size.related_locations,
        text_bytes: size.text_bytes,
        source_bytes: size.source_bytes,
    };
    protocol.limits.sources = 1;
    let frame = protocol.encode_response(Err(&original)).unwrap();
    assert!(protocol.decode_response(&frame).unwrap().is_err());
    for dimension in 0..7 {
        let mut limited = protocol.clone();
        match dimension {
            0 => limited.limits.diagnostics.diagnostics -= 1,
            1 => limited.limits.diagnostics.depth -= 1,
            2 => limited.limits.diagnostics.call_frames -= 1,
            3 => limited.limits.diagnostics.related_locations -= 1,
            4 => limited.limits.diagnostics.text_bytes -= 1,
            5 => limited.limits.diagnostics.source_bytes -= 1,
            _ => limited.limits.sources = 0,
        }
        assert!(limited.decode_response(&frame).is_err());
        assert!(limited.encode_response(Err(&original)).is_err());
    }
    let control = OperationControl::default();
    let ResponsePlan::Error(plan) = protocol.response_plan(&frame, &control).unwrap() else {
        panic!()
    };
    let summary = plan.rejected(limit("diagnostic text bytes", 0), None);
    assert!(summary.is_emergency());
    assert_eq!(summary.causes[0].code(), original.code());
    let BWErr::NativeError(detail) = summary.causes[0].error.as_ref() else {
        panic!()
    };
    assert_eq!(detail.len(), 256);
    assert_eq!(
        summary.causes[0].omissions.as_ref().unwrap().detail_fields,
        1
    );
    assert!(summary.causes[0].span.is_none());
}

#[test]
fn stream_helpers_require_eof_complete_writes_and_flush() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Ok(0)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    struct Flush;
    impl Write for Flush {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::Other.into())
        }
    }
    let protocol = WorkerProtocol::default();
    let frame = protocol.encode_request(&[Literal::None]).unwrap();
    assert!(matches!(
        protocol.read_request(&mut &frame[..]).unwrap().as_slice(),
        [Literal::None]
    ));
    let mut extra = frame.clone();
    extra.push(0);
    assert!(protocol.read_request(&mut &extra[..]).is_err());
    assert!(protocol
        .read_request(&mut &frame[..frame.len() - 1])
        .is_err());
    assert!(protocol
        .write_response(&mut Broken, Ok(&Literal::None))
        .is_err());
    assert!(protocol
        .write_response(&mut Flush, Ok(&Literal::None))
        .is_err());
}

#[test]
fn worker_sdk_validates_both_signature_sides_and_contains_panics() {
    let protocol = WorkerProtocol::default();
    let signature = StatementSignature::native("Echo |x|")
        .unwrap()
        .parameter("x", ValueKind::Int)
        .unwrap()
        .returns(ValueKind::Int);
    let frame = protocol.encode_request(&[Literal::Int(9)]).unwrap();
    let mut output = Vec::new();
    protocol
        .serve_once(&signature, &mut &frame[..], &mut output, |mut values| {
            Ok(values.remove(0))
        })
        .unwrap();
    assert!(matches!(
        protocol.decode_response(&output).unwrap().unwrap(),
        Literal::Int(9)
    ));
    for mode in 0..4 {
        let frame = protocol
            .encode_request(if mode == 0 {
                &[]
            } else if mode == 1 {
                &[Literal::None]
            } else {
                &[Literal::Int(1)]
            })
            .unwrap();
        let mut output = Vec::new();
        protocol
            .serve_once(&signature, &mut &frame[..], &mut output, |_| {
                if mode == 3 {
                    panic!("callback panic")
                } else {
                    Ok(Literal::None)
                }
            })
            .unwrap();
        let code = protocol
            .decode_response(&output)
            .unwrap()
            .unwrap_err()
            .code();
        assert_eq!(
            code.as_str(),
            match mode {
                0 => "BW2004",
                3 => "BW4003",
                _ => "BW3003",
            }
        );
    }
}

#[test]
fn cancellation_between_scan_and_build_cannot_panic() {
    let protocol = WorkerProtocol::default();
    let control = OperationControl::default();
    let frame = protocol.encode_response(Ok(&Literal::Int(1))).unwrap();
    assert!(protocol.response_plan(&frame, &control).is_ok());
    control.cancel();
    assert!(matches!(protocol.build_value(&frame), Literal::Int(1)));
    assert!(protocol.response_plan(&frame, &control).is_err());
}

#[test]
fn diagnostic_wire_references_spans_and_unused_tables_are_validated() {
    fn number(body: &mut Vec<u8>, value: u64) {
        body.extend_from_slice(&value.to_le_bytes());
    }
    fn string(body: &mut Vec<u8>, value: &str) {
        number(body, value.len() as u64);
        body.extend_from_slice(value.as_bytes());
    }
    for case in 0..8 {
        let mut body = Vec::new();
        number(&mut body, 1);
        string(&mut body, "name");
        string(&mut body, "é");
        number(&mut body, 1);
        body.extend_from_slice(&(if case == 1 { 9999u16 } else { 4002u16 }).to_le_bytes());
        string(&mut body, "detail");
        number(&mut body, u64::from(case == 2));
        string(&mut body, "source");
        body.push(u8::from(case != 3));
        if case != 3 {
            number(&mut body, u64::from(case == 4));
            number(&mut body, if case == 5 { 1 } else { 0 });
            number(&mut body, if case == 6 { 3 } else { 2 });
        }
        number(&mut body, 0);
        number(&mut body, 0);
        body.push(0);
        number(&mut body, u64::from(case == 7));
        let mut frame = response(&body);
        frame[6] = 2;
        let result = WorkerProtocol::default().decode_response(&frame);
        assert_eq!(result.is_ok(), case == 0, "case {case}");
    }
}

#[test]
fn source_identity_is_distinct_even_with_equal_source_contents() {
    let mut original = error();
    let another = error();
    original.causes.push(another);
    let protocol = WorkerProtocol::default();
    let frame = protocol.encode_response(Err(&original)).unwrap();
    let decoded = protocol.decode_response(&frame).unwrap().unwrap_err();
    assert_eq!(
        protocol.limits.diagnostics.check(&original).unwrap(),
        protocol.limits.diagnostics.check(&decoded).unwrap()
    );
    assert!(!Arc::ptr_eq(
        decoded.span.as_ref().unwrap().source(),
        decoded.causes[1].span.as_ref().unwrap().source()
    ));
}
