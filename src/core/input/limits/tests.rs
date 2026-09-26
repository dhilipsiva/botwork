use super::*;

#[test]
fn decoded_lengths_match_json_escaping_and_utf16_boundaries() {
    for code in [
        0, 1, 8, 9, 10, 13, 32, 34, 47, 92, 127, 128, 255, 0x7ff, 0x800, 0xfffd, 0x10000, 0x1f642,
        0x10ffff,
    ] {
        let character = char::from_u32(code).unwrap();
        let text = character.to_string();
        let json = serde_json::to_string(&text).unwrap();
        assert_eq!(string_extent(json.as_bytes(), 0), (json.len(), text.len()));
        let mut utf16 = [0; 2];
        let units = character.encode_utf16(&mut utf16);
        let encoded = format!(
            "\"{}\"",
            units
                .iter()
                .map(|unit| format!("\\u{unit:04X}"))
                .collect::<String>()
        );
        assert_eq!(
            string_extent(encoded.as_bytes(), 0),
            (encoded.len(), text.len())
        );
    }
    let text = (0..=127)
        .map(|code| char::from_u32(code).unwrap())
        .collect::<String>();
    let json = serde_json::to_string(&text).unwrap();
    assert_eq!(string_extent(json.as_bytes(), 0), (json.len(), text.len()));
}

#[test]
fn preflight_counts_all_raw_keys_values_and_discarded_containers() {
    let limits = InputLimits {
        raw_nodes: 10,
        ..InputLimits::default()
    };
    let mut budget = Budget::new(&limits).unwrap();
    budget
        .preflight("raw", r#"{"x":[true,false,null,{},[]],"x":0}"#)
        .unwrap();
    assert_eq!(budget.nodes, 10);
    assert!(budget
        .preflight("next", "0")
        .unwrap_err()
        .to_string()
        .contains("input raw nodes"));
}

#[test]
fn raw_nesting_bound_remains_independent_of_value_conversion() {
    let limits = InputLimits::default();
    for depth in [MAX_JSON_DEPTH, MAX_JSON_DEPTH + 1] {
        let text = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        let result = Budget::new(&limits).unwrap().preflight("depth", &text);
        if depth == MAX_JSON_DEPTH {
            result.unwrap();
        } else {
            assert_eq!(
                result.unwrap_err().code(),
                crate::core::diagnostic::DiagnosticCode::Input
            );
        }
    }
}

#[test]
fn cumulative_bytes_reject_overflow_without_wrapping() {
    let limits = InputLimits {
        source_bytes: usize::MAX,
        total_bytes: usize::MAX,
        ..InputLimits::default()
    };
    let mut budget = Budget::new(&limits).unwrap();
    budget.bytes("first", usize::MAX).unwrap();
    assert_eq!(budget.remaining(), 0);
    assert!(budget.bytes("second", 1).is_err());
    assert_eq!(budget.bytes, usize::MAX);
}

#[test]
fn malformed_string_scanning_never_slices_inside_utf8() {
    for text in [
        r#""\ué""#,
        r#""\u""#,
        r#""\uD800\uZZZZ""#,
        r#""\uD800\uD800""#,
        r#""\uDC00""#,
        "\"unfinished\\",
    ] {
        let (end, _) = string_extent(text.as_bytes(), 0);
        assert!(end <= text.len());
        assert!(text.is_char_boundary(end));
    }
}

#[test]
fn raw_visitors_preserve_typed_resource_failures_and_duplicate_values() {
    let limits = InputLimits {
        variables: 1,
        values: ValueLimits {
            entries: 1,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    let map = crate::core::input::raw::map("map", "$", r#"{"x":1,"x":2}"#, &limits, true).unwrap();
    assert_eq!(map["x"].get(), "2");
    for error in [
        crate::core::input::raw::map("map", "$", r#"{"x":1,"y":2}"#, &limits, true).unwrap_err(),
        crate::core::input::raw::array("array", "$", "[1,2]", &limits).unwrap_err(),
    ] {
        assert_eq!(
            error.code(),
            crate::core::diagnostic::DiagnosticCode::ResourceLimit
        );
    }
    assert_eq!(
        crate::core::input::raw::array("array", "$", "[1,]", &InputLimits::default())
            .unwrap_err()
            .code(),
        crate::core::diagnostic::DiagnosticCode::Input
    );
}
