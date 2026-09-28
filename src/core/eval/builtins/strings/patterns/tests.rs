use super::*;

#[test]
fn search_accounting_accepts_the_exact_boundary_and_rejects_overflow() {
    let context = Context::default();
    let mut used = 0;
    charge(&context, &mut used, SEARCH_BYTES / 4 - 1, 4).unwrap();
    assert_eq!(used, SEARCH_BYTES);
    assert!(charge(&context, &mut used, 0, 1).is_err());
    assert_eq!(used, SEARCH_BYTES);
    for (used, bytes, multiplier) in [
        (0, usize::MAX, 1),
        (0, usize::MAX / 2, 4),
        (usize::MAX, 0, 1),
    ] {
        assert!(charge(&Context::default(), &mut { used }, bytes, multiplier).is_err());
    }
}

#[test]
fn pattern_length_limit_includes_the_exact_maximum_without_retaining_a_cache() {
    let pattern = "a".repeat(PATTERN_BYTES);
    assert!(compile(&Context::default(), &pattern).is_ok());
    assert!(compile(&Context::default(), &(pattern + "a"))
        .unwrap_err()
        .into_diagnostic()
        .to_string()
        .contains("regex pattern bytes"));
}
