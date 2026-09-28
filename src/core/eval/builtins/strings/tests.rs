use super::*;

#[test]
fn scalar_lengths_require_exact_int_conversion_without_large_allocations() {
    let context = Context::default();
    assert_eq!(length_int(&context, i32::MAX as usize).unwrap(), i32::MAX);
    for count in [i32::MAX as usize + 1, usize::MAX] {
        assert_eq!(
            length_int(&context, count)
                .unwrap_err()
                .into_diagnostic()
                .code(),
            Code::Arithmetic
        );
    }
}
