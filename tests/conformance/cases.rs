#[derive(Clone)]
pub enum Input {
    Script(&'static str),
    NonFiniteHost,
}

#[derive(Clone)]
pub struct Case {
    pub id: &'static str,
    pub positive: &'static [&'static str],
    pub invalid: &'static [&'static str],
    pub boundary: &'static [&'static str],
    pub input: Input,
    pub stdout: &'static str,
    pub error: Option<&'static str>,
    pub code: Option<&'static str>,
}

fn success(
    id: &'static str,
    source: &'static str,
    stdout: &'static str,
    positive: &'static [&'static str],
    boundary: &'static [&'static str],
) -> Case {
    Case {
        id,
        positive,
        invalid: &[],
        boundary,
        input: Input::Script(source),
        stdout,
        error: None,
        code: None,
    }
}

fn failure(
    id: &'static str,
    source: &'static str,
    stdout: &'static str,
    code: &'static str,
    error: &'static str,
    invalid: &'static [&'static str],
) -> Case {
    Case {
        id,
        positive: &[],
        invalid,
        boundary: &[],
        input: Input::Script(source),
        stdout,
        error: Some(error),
        code: Some(code),
    }
}

pub fn cases() -> Vec<Case> {
    vec![
        success("values", include_str!("values.botwork"), concat!(
            "[none, true, -1, 1.5, \"é\", [], {}]\n[1, 2]\n{\"a\": 2, \"z\": 3}\nfalse\n",
            "[none, none, [\"ok\"]]\n"), &["E2", "V1", "V7", "C2"], &["V7", "C2"]),
        success("control", include_str!("control.botwork"), "[7, 4, 2]\niterator absent\n[false, true]\n",
            &["E1", "E4", "S3", "C1", "F1"], &["S3", "C1"]),
        success("scope", include_str!("scope.botwork"), "[10, 100, [1, 100]]\n10\n11\n11\nlocal discarded\n7\n",
            &["E3", "S1", "S2"], &["E3", "S1", "S2"]),
        success("numeric", include_str!("numeric.botwork"), concat!(
            "[-2147483648, 2147483647, 512, -4, 4, 0.25]\n[true, false, true]\n",
            "[true, true]\nliteral range\narithmetic overflow\n"), &["V2", "V5"], &["V1", "V2", "V5"]),
        success("collections", include_str!("collections.botwork"), concat!(
            "[3, 7, 8, 9, 6]\n[true, true, true, true, false]\n[true, true, false]\n",
            "end is out of bounds\nempty is out of bounds\n"), &["V3", "V4", "V6"], &["V3", "V4", "V6"]),
        success("names", include_str!("names.botwork"), "தமிழ்\ncollision preserves original\n7\n[1, 2, 3, 4]\n8\n",
            &["L3", "L4"], &["L3", "L4"]),
        success("layout", include_str!("../../examples/14-multiline-layout.botwork"), "[2, 3]\né🙂 | # { }\n5\nrecovered\n",
            &["L1", "L2"], &[]),
        success("empty", "", "", &[], &["L1", "C2"]),
        success("crlf-comments", "\t# line\r\n### block\r\ncomment ###\r\n\r\nLog |\"# | { }\"|\r\n", "# | { }\n", &[], &["L1", "L2"]),
        success("recovery", include_str!("recovery.botwork"),
            "before\ncompleted effect\n7\nouter handler\nnot hoisted\nregistered later\n[false, true]\n",
            &["F2", "F3"], &["E1", "E2", "E4", "F1", "F2"]),
        failure("incomplete-continuation", include_str!("../fixtures/invalid-continuation.botwork"), "", "BW1001", "Parsing error:", &["E1", "L1"]),
        failure("unclosed-comment", include_str!("../fixtures/unclosed-block-comment.botwork"), "", "BW1001", "Parsing error:", &["L2"]),
        failure("resolve-before-arguments", "Unknown |missing_argument|", "", "BW2002", "Statement not defined: Unknown", &["E3", "S1"]),
        failure("return-operand-order", "Fail { Return |missing_first + missing_second| }\nLog |\"before\"|\nFail\nLog |\"unreachable\"|", "before\n", "BW2001", "Variable not defined: missing_first", &["E2", "V1", "C2", "F2"]),
        failure("strict-condition", "If |1| { Log |\"unreachable\"| }", "", "BW3003", "If requires a boolean condition", &["E4"]),
        failure("comparison-chain", "Log |1 < 2 < 3|", "", "BW3003", "Operation performed on incompatible types:", &["V2", "V5"]),
        failure("missing-path-key", "Log |{a: 1}.missing|", "", "BW3004", "map key does not exist", &["V3"]),
        failure("computed-key-type", "Log |{a: 1}[0]|", "", "BW3004", "map key must be a string", &["V4"]),
        failure("unsupported-operator", "Log |[1] - [1]|", "", "BW3003", "Operation performed on incompatible types:", &["V7"]),
        failure("duplicate-parameter", include_str!("../fixtures/duplicate-parameter.botwork"), "", "BW1003", "Duplicate parameter `x`", &["L3"]),
        failure("invalid-unicode", include_str!("../fixtures/invalid-unicode-identifier.botwork"), "", "BW1001", "Parsing error:", &["L4"]),
        failure("private-caller", "Read { Return |private| }\nOuter { |private| = |7|\nRead\n}\nOuter", "", "BW2001", "Variable not defined: private", &["S1"]),
        failure("expired-definition", "Outer { Local {} }\nOuter\nLocal", "", "BW2002", "Statement not defined: Local", &["S2"]),
        failure("non-array-iteration", "For |item| In |{}| { Log |\"unreachable\"| }", "", "BW3003", "For requires an array", &["S3"]),
        failure("invalid-control", include_str!("../fixtures/invalid-control-unused-break.botwork"), "", "BW1002", "Break requires", &["C1"]),
        failure("required-catch", include_str!("../fixtures/missing-catch.botwork"), "", "BW1001", "Parsing error:", &["F1"]),
        failure("diagnostic-stack", include_str!("../fixtures/diagnostic-stack.botwork"), "", "BW2001", "diagnostic-stack.botwork:2:17", &["F3"]),
        Case { boundary: &["F3"], ..failure("diagnostic-handler", include_str!("../fixtures/diagnostic-handler.botwork"), "", "BW2001", "while handling:", &["F3"]) },
        Case { id: "structural-host-invalid", positive: &[], invalid: &["V6"], boundary: &[],
            input: Input::NonFiniteHost, stdout: "", code: Some("BW3002"), error: Some("Non-finite floating-point operand") },
    ]
}
