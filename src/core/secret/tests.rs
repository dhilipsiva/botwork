use super::*;
use std::collections::HashMap;

#[test]
fn nested_string_and_number_leaves_are_masked_in_every_spelling() {
    let secrets = Secrets::default();
    assert!(secrets.is_empty());
    let value = Literal::Map(HashMap::from([
        ("token".to_owned(), Literal::String("s3cr\"et\n".into())),
        (
            "pins".to_owned(),
            Literal::Array(vec![Literal::Int(4812), Literal::Float(2.5)]),
        ),
        ("enabled".to_owned(), Literal::Bool(true)),
        ("none".to_owned(), Literal::None),
    ]));
    secrets.add(&value);
    assert!(!secrets.is_empty());
    for (text, masked) in [
        ("raw s3cr\"et\n end", "raw *** end"),
        (r#"json "s3cr\"et\n" end"#, r#"json "***" end"#),
        ("pin 4812, rate 2.5", "pin ***, rate ***"),
        ("true and null stay", "true and null stay"),
        ("token is a key, not a value", "token is a key, not a value"),
    ] {
        assert_eq!(secrets.redact(text), masked, "{text:?}");
    }
    let quoted = format!("[{}]", QuotedString("s3cr\"et\n"));
    assert_eq!(secrets.redact(&quoted), "[\"***\"]");
    // Control characters escape differently in JSON and in quoted values.
    secrets.add(&Literal::String("ctl\u{1}x".into()));
    assert_eq!(
        secrets.redact(r#"{"key":"ctl\u0001x"}"#),
        r#"{"key":"***"}"#
    );
    assert_eq!(secrets.redact(r#"["ctl\u{1}x"]"#), r#"["***"]"#);
    // DEL is raw in JSON but escaped in quoted collection values.
    secrets.add(&Literal::String("del\u{7f}ete".into()));
    let quoted = QuotedString("del\u{7f}ete").to_string();
    assert_eq!(quoted, "\"del\\u{7f}ete\"");
    assert_eq!(secrets.redact(&quoted), "\"***\"");
    assert_eq!(secrets.redact("del\u{7f}ete"), "***");
}

#[test]
fn longer_secrets_are_masked_whole_and_clones_share_the_registry() {
    let secrets = Secrets::default();
    let shared = secrets.clone();
    secrets.add(&Literal::String("abc".into()));
    shared.add(&Literal::String("abcdef".into()));
    assert_eq!(secrets.redact("abcdef abc"), "*** ***");
    assert!(matches!(secrets.redact("nothing here"), Cow::Borrowed(_)));
    secrets.add(&Literal::String(String::new()));
    assert_eq!(secrets.redact(""), "");
    assert_eq!(format!("{secrets:?}"), "Secrets(2 texts)");
}

#[test]
fn the_writer_masks_secrets_split_across_writes() {
    let secrets = Secrets::default();
    secrets.add(&Literal::String("hunter2".into()));
    let mut output = Vec::new();
    {
        let mut writer = secrets.writer(&mut output);
        writer.write_all(b"pass: hun").unwrap();
        writer.write_all(b"ter2\n").unwrap();
        writer.flush().unwrap();
        writer.write_all(b"again hunter2").unwrap();
    }
    assert_eq!(String::from_utf8(output).unwrap(), "pass: ***\nagain ***");
    let mut plain = Vec::new();
    Secrets::default()
        .writer(&mut plain)
        .write_all(b"unbuffered")
        .unwrap();
    assert_eq!(plain, b"unbuffered");
}

#[test]
fn masked_copies_hide_secret_leaves_keys_and_url_spellings() {
    let secrets = Secrets::default();
    assert!(secrets.mask_value(&Literal::String("x".into())).is_none());
    secrets.add(&Literal::Array(vec![
        Literal::String("pa ss&word".into()),
        Literal::Int(4812),
    ]));
    assert_eq!(secrets.redact("q=pa+ss%26word&n=1"), "q=***&n=1");
    let value = Literal::Map(HashMap::from([
        ("pa ss&word".to_owned(), Literal::Int(1)),
        (
            "nested".to_owned(),
            Literal::Array(vec![Literal::Int(4812), Literal::String("keep".into())]),
        ),
    ]));
    let Some(Literal::Map(masked)) = secrets.mask_value(&value) else {
        panic!("the map holds secrets");
    };
    assert!(
        matches!(masked.get("***"), Some(Literal::Int(1))),
        "keys are masked"
    );
    let Some(Literal::Array(nested)) = masked.get("nested") else {
        panic!("nested array");
    };
    assert!(matches!(&nested[0], Literal::String(text) if text == MASK));
    assert!(matches!(&nested[1], Literal::String(text) if text == "keep"));
    assert!(secrets.mask_value(&Literal::Bool(true)).is_none());
    assert!(secrets
        .mask_value(&Literal::String("public".into()))
        .is_none());
}
