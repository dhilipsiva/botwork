use super::*;

#[test]
fn canonical_duration_round_trips_full_signed_range() {
    let context = Context::default();
    for value in [
        i128::MIN,
        i128::MIN + 1,
        -DAY,
        -SECOND - 1,
        -1,
        0,
        1,
        SECOND + 1,
        DAY,
        i128::MAX,
    ] {
        let mut text = String::new();
        canonical(&mut text, value).unwrap();
        assert_eq!(parse(&context, &text).unwrap(), value, "{text}");
    }
    for sign in ["", "-"] {
        assert!(parse(
            &context,
            &format!("{sign}PT170141183460469231731687303715.884105729S")
        )
        .is_err());
    }
}

#[test]
fn decimal_seconds_keep_sign_and_fraction_below_one_second() {
    for (value, expected) in [
        (-1, "-0.000000001"),
        (10, "0.00000001"),
        (-1_200_000_000, "-1.2"),
        (0, "0"),
    ] {
        let mut text = String::new();
        seconds(&mut text, value).unwrap();
        assert_eq!(text, expected);
    }
}
