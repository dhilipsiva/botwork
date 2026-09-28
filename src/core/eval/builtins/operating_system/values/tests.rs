use super::*;

#[test]
fn directory_names_are_sorted_by_exact_utf8_without_normalization() {
    let context = Context::default();
    let mut names = StringArray::new(&context).unwrap();
    for name in ["é", "z", ".hidden", "é"] {
        names.push(OsStr::new(name)).unwrap();
    }
    assert_eq!(
        names.sorted().to_string(),
        "[\".hidden\", \"é\", \"z\", \"é\"]"
    );
}

#[test]
fn binary_output_metrics_check_empty_exact_and_overflow_boundaries() {
    let context = Context::default();
    assert_eq!(
        binary_size(&context, 0).unwrap(),
        ValueSize {
            nodes: 1,
            depth: 1,
            payload_bytes: 0
        }
    );
    assert_eq!(
        binary_size(&context, 3).unwrap(),
        ValueSize {
            nodes: 4,
            depth: 2,
            payload_bytes: 12
        }
    );
    assert!(binary_size(&context, usize::MAX).is_err());
}
