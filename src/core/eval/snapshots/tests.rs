use super::*;

#[test]
fn frame_clones_rebuild_live_entries_without_spare_capacity() {
    let mut context = Context::default();
    context.init_statements();
    context.set_variable("x", Literal::Int(7)).unwrap();
    let frame = &mut context.frames[0];
    frame.variables.reserve(16_384);
    frame.statements.reserve(16_384);
    frame.namespaces.reserve(16_384);
    let copy = frame.clone();
    assert_eq!(copy.variables.len(), 1);
    assert!(copy.variables.capacity() <= 7);
    assert_eq!(copy.statements.len(), 57);
    assert!(copy.statements.capacity() <= 114);
    assert_eq!(copy.namespaces.capacity(), 0);
    assert!(Arc::ptr_eq(&copy.variables["x"], &frame.variables["x"]));
}
