use botwork::core::{
    grammar::{Literal, Operate, Rule},
    run::{Engine, RunLimits, RunOptions, RunOutcome},
    value_limits::ValueLimits,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct ObservedAllocator;
thread_local! {
    static THRESHOLD:Cell<usize>=const {Cell::new(usize::MAX)};
    static LARGE:Cell<usize>=const {Cell::new(0)};
}
fn record(bytes: usize) {
    let _ = THRESHOLD.try_with(|limit| {
        if bytes >= limit.get() {
            let _ = LARGE.try_with(|count| count.set(count.get() + 1));
        }
    });
}
unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

fn observe<T>(threshold: usize, action: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            THRESHOLD.with(|value| value.set(usize::MAX));
        }
    }
    LARGE.with(|value| value.set(0));
    THRESHOLD.with(|value| value.set(threshold));
    let reset = Reset;
    let result = action();
    drop(reset);
    (result, LARGE.with(Cell::get))
}

#[test]
fn rejected_concatenation_does_not_allocate_result_or_format_operand_payloads() {
    let length = 64 * 1024;
    let left = Literal::String("a".repeat(length));
    let right = Literal::String("b".repeat(length));
    let limits = ValueLimits {
        string_bytes: length,
        ..ValueLimits::default()
    };
    let (result, large) = observe(length, || {
        Rule::plus.operate_binary_bounded(left, right, &limits)
    });
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("value string bytes"));
    assert_eq!(large, 0);
    let left = Literal::Array(vec![Literal::None; 4096]);
    let right = Literal::Array(vec![Literal::None; 4096]);
    let limits = ValueLimits {
        entries: 4096,
        ..ValueLimits::default()
    };
    let (result, large) = observe(4096 * std::mem::size_of::<Literal>(), || {
        Rule::plus.operate_binary_bounded(left, right, &limits)
    });
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("value container entries"));
    assert_eq!(large, 0);
}

#[test]
fn accepted_concatenation_reuses_owned_strings_and_array_elements() {
    let mut text = String::with_capacity(64);
    text.push_str("left");
    let pointer = text.as_ptr();
    let result = Rule::plus
        .operate_binary(Literal::String(text), Literal::String("right".into()))
        .unwrap();
    let Literal::String(result) = result else {
        panic!("string")
    };
    assert_eq!(result, "leftright");
    assert_eq!(result.as_ptr(), pointer);
    let text = "payload".repeat(100);
    let pointer = text.as_ptr();
    let result = Rule::plus
        .operate_binary(
            Literal::Array(vec![Literal::String(text)]),
            Literal::Array(vec![]),
        )
        .unwrap();
    let Literal::Array(result) = result else {
        panic!("array")
    };
    let Literal::String(text) = &result[0] else {
        panic!("string")
    };
    assert_eq!(text.as_ptr(), pointer);
}

fn recording_engine() -> (Engine, Arc<Mutex<Vec<i32>>>) {
    let events = Arc::new(Mutex::new(vec![]));
    let observed = events.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Mark |value|", move |arguments, _| {
            let Literal::Int(value) = arguments[0] else {
                panic!("integer")
            };
            observed.lock().unwrap().push(value);
            Ok(Literal::Int(value))
        })
        .unwrap();
    (engine, events)
}

fn options(values: ValueLimits) -> RunOptions {
    RunOptions {
        limits: RunLimits {
            values,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

#[test]
fn known_container_width_and_keys_are_checked_before_child_effects() {
    for (source, limits, resource) in [
        (
            "|x| = |[@{Mark |1|},@{Mark |2|},@{Mark |3|}]|",
            ValueLimits {
                entries: 2,
                ..ValueLimits::default()
            },
            "value container entries",
        ),
        (
            "|x| = |{a:@{Mark |1|},b:@{Mark |2|}}|",
            ValueLimits {
                entries: 1,
                ..ValueLimits::default()
            },
            "value container entries",
        ),
        (
            "|x| = |{long:@{Mark |1|}}|",
            ValueLimits {
                key_bytes: 3,
                ..ValueLimits::default()
            },
            "value key bytes",
        ),
        (
            "|x| = |{aaaa:@{Mark |1|},bbbb:@{Mark |2|}}|",
            ValueLimits {
                payload_bytes: 7,
                ..ValueLimits::default()
            },
            "value payload bytes",
        ),
        (
            "|x| = |[@{Mark |1|},@{Mark |2|}]|",
            ValueLimits {
                nodes: 2,
                ..ValueLimits::default()
            },
            "value nodes",
        ),
    ] {
        let (engine, events) = recording_engine();
        let result = engine.run_source("preflight", source, options(limits));
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert!(result.result.unwrap_err().to_string().contains(resource));
        assert!(events.lock().unwrap().is_empty());
        assert!(result.variables.is_empty());
    }
}

#[test]
fn dynamic_collection_growth_stops_before_later_children() {
    for source in [
        "|x| = |[@{Mark |1|},@{Mark |2|},@{Mark |3|},@{Mark |4|}]|",
        "|x| = |{a:@{Mark |1|},b:@{Mark |2|},c:@{Mark |3|},d:@{Mark |4|}}|",
    ] {
        let (engine, events) = recording_engine();
        let result = engine.run_source(
            "growth",
            source,
            options(ValueLimits {
                payload_bytes: if source.contains('{') && source.contains("a:") {
                    12
                } else {
                    8
                },
                ..ValueLimits::default()
            }),
        );
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert_eq!(*events.lock().unwrap(), vec![1, 2, 3]);
        assert!(result.variables.is_empty());
    }
}

#[test]
fn duplicate_map_keys_preserve_all_effects_and_replace_retained_metrics() {
    let (engine, events) = recording_engine();
    let result = engine.run_source(
        "duplicates",
        "|x| = |{k:@{Mark |1|},\"k\":@{Mark |2|}}|",
        options(ValueLimits {
            nodes: 2,
            entries: 1,
            payload_bytes: 5,
            ..ValueLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(*events.lock().unwrap(), vec![1, 2]);
    assert_eq!(result.variables["x"].to_string(), "{\"k\": 2}");
    let result = Engine::default().run_source(
        "depth",
        "|x| = |[{k:[[1]],k:[]}]|",
        options(ValueLimits {
            depth: 4,
            ..ValueLimits::default()
        }),
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
}

#[test]
fn cumulative_nodes_depth_and_concat_payload_are_checked() {
    for (source, limits, resource) in [
        (
            "|x| = |[[1],[2]]|",
            ValueLimits {
                nodes: 4,
                ..ValueLimits::default()
            },
            "value nodes",
        ),
        (
            "|x| = |[[[]]]|",
            ValueLimits {
                depth: 2,
                ..ValueLimits::default()
            },
            "value depth",
        ),
        (
            "|x| = |[[1]]+[[2]]|",
            ValueLimits {
                nodes: 4,
                ..ValueLimits::default()
            },
            "value nodes",
        ),
        (
            "|x| = |\"abcd\"+\"efgh\"|",
            ValueLimits {
                payload_bytes: 7,
                ..ValueLimits::default()
            },
            "value payload bytes",
        ),
        (
            "|x| = |\"é\"+\"é\"|",
            ValueLimits {
                string_bytes: 3,
                ..ValueLimits::default()
            },
            "value string bytes",
        ),
    ] {
        let result = Engine::default().run_source("bounds", source, options(limits));
        assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
        assert!(result.result.unwrap_err().to_string().contains(resource));
    }
}

#[test]
fn doubling_stops_before_assignment_and_keeps_the_original_binding() {
    let mut options = options(ValueLimits {
        string_bytes: 16,
        ..ValueLimits::default()
    });
    options.variables = BTreeMap::from([("value".into(), Literal::String("x".repeat(16)))]);
    let result = Engine::default().run_source(
        "doubling",
        "Try { |value| = |value+value| } Catch { |handled| = |true| }",
        options,
    );
    assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(result.variables["value"].to_string(), "x".repeat(16));
    assert!(!result.variables.contains_key("handled"));
}

#[test]
fn accepted_string_concatenation_does_not_format_unused_error_payloads() {
    let threshold = 64 * 1024;
    let mut text = String::with_capacity(threshold * 2);
    text.push_str(&"a".repeat(threshold));
    let pointer = text.as_ptr();
    let left = Literal::String(text);
    let right = Literal::String("b".into());
    let (result, large) = observe(threshold, || Rule::plus.operate_binary(left, right));
    let Literal::String(result) = result.unwrap() else {
        panic!("string")
    };
    assert_eq!(result.as_ptr(), pointer);
    assert_eq!(large, 0);
}

#[test]
fn oversized_source_strings_and_keys_are_rejected_before_payload_copying() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticCode,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    for (source, limits) in [
        (
            format!("|x| = |\"{}\"|", "a".repeat(length)),
            ValueLimits {
                string_bytes: 3,
                ..ValueLimits::default()
            },
        ),
        (
            format!("|x| = |{{\"{}\":1}}|", "a".repeat(length)),
            ValueLimits {
                key_bytes: 3,
                ..ValueLimits::default()
            },
        ),
    ] {
        let program = Program::parse("copy", &source).unwrap();
        let mut context = Context::with_limits(RunLimits {
            values: limits,
            ..RunLimits::default()
        })
        .unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
        assert_eq!(large, 0);
    }
}
