use super::*;

#[test]
fn path_workspace_admits_the_exact_boundary_and_rejects_overflow() {
    assert!(bounded(&Context::default(), Some(MAX_PATH_BYTES)).is_ok());
    assert!(bounded(&Context::default(), Some(MAX_PATH_BYTES + 1)).is_err());
    assert!(bounded(&Context::default(), None).is_err());
}

#[test]
fn removal_requires_a_final_name_without_executing_any_removal() {
    let context = Context::default();
    for value in ["", "..", "outside/.."] {
        assert!(validate_removal(&context, Path::new(value)).is_err());
    }
    #[cfg(unix)]
    assert!(validate_removal(&context, Path::new("/")).is_err());
    assert!(validate_removal(&context, Path::new("outside")).is_ok());
}
