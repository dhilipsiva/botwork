use super::*;
use crate::core::{csv::Fields, value_limits::ValueLimits};

#[test]
fn grown_admission_equals_the_converted_value_size() {
    for json in [
        "null",
        "true",
        "-7",
        "1.5e3",
        r#""é""#,
        "[]",
        "{}",
        r#"[1, [2.0, [null, [false, "x"]]]]"#,
        r#"{"a": {"b": [1, {"c": "deep"}]}, "": ""}"#,
        r#"{"k": "discarded", "k": [1, 2]}"#,
    ] {
        let (mut nodes, mut bytes) = (0, 0);
        let value = input::parse_document("test", json, &InputLimits::default(), &mut |n, b| {
            nodes += n;
            bytes += b;
            Ok(())
        })
        .unwrap();
        let size = ValueLimits::default().check(&value).unwrap();
        assert_eq!((nodes, bytes), (size.nodes, size.payload_bytes), "{json}");
    }
}

#[test]
fn rejected_growth_stops_conversion_with_the_admission_error() {
    let mut calls = 0;
    let error = input::parse_document("test", "[1, 2, 3]", &InputLimits::default(), &mut |_, _| {
        calls += 1;
        if calls == 3 {
            Err(BWErr::ResourceLimit {
                resource: "temporary value nodes",
                limit: 2,
            })
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(calls, 3, "no node after the rejected one is admitted");
    assert!(matches!(
        &*error.error,
        BWErr::ResourceLimit {
            resource: "temporary value nodes",
            ..
        }
    ));
}

#[test]
fn csv_positions_follow_quoted_line_breaks_and_crlf() {
    let text = "a,b\r\n\"x\ny\",2\r\n\"é\"z";
    let mut fields = Fields::new(text);
    let mut seen = Vec::new();
    while let Some(field) = fields.next() {
        match field {
            Ok(field) => seen.push((field.text.into_owned(), field.last)),
            Err(error) => {
                seen.push((format!("{}:{}", error.line, error.column), true));
                assert_eq!(&text[error.offset..], "z");
                break;
            }
        }
    }
    assert_eq!(
        seen,
        [
            ("a".into(), false),
            ("b".into(), true),
            ("x\ny".into(), false),
            ("2".into(), true),
            ("4:4".into(), true),
        ]
    );
}

#[test]
fn canonical_json_escapes_and_orders_deterministically() {
    assert_eq!(
        JsonString("\"\\\n\r\t\u{8}\u{c}\u{1}\u{7f}é").to_string(),
        // Only U+0000-U+001F must be escaped; DEL passes through unchanged.
        concat!(r#""\"\\\n\r\t\b\f\u0001"#, "\u{7f}é\"")
    );
    let map = Literal::Map(
        [
            ("b".to_owned(), Literal::Float(1.0)),
            (
                "a".to_owned(),
                Literal::Array(vec![Literal::None, Literal::Int(-1)]),
            ),
        ]
        .into(),
    );
    assert_eq!(Canonical(&map).to_string(), r#"{"a":[null,-1],"b":1.0}"#);
    assert_eq!(without_signature("\u{feff}\u{feff}x"), "\u{feff}x");
}
