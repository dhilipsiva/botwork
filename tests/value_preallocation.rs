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
#[path = "support/worker_panic.rs"]
mod worker_panic;
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
    // The default panic hook captures a backtrace when RUST_BACKTRACE is set, and
    // those allocations belong to the hook, not the library. Print panics without
    // one for the whole test binary.
    static QUIET: std::sync::Once = std::sync::Once::new();
    QUIET.call_once(|| std::panic::set_hook(Box::new(|info| eprintln!("{info}"))));
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
fn worker_frames_and_decoded_values_are_admitted_before_payload_allocation() {
    use botwork::core::worker::protocol::WorkerProtocol;
    let value = Literal::String("x".repeat(100_000));
    let protocol = WorkerProtocol::default();
    for allowed in [false, true] {
        let mut configured = protocol.clone();
        if !allowed {
            configured.limits.frame_bytes = 0;
        }
        let (result, copies) = observe(100_000, || {
            configured.encode_request(std::slice::from_ref(&value))
        });
        assert_eq!(result.is_ok(), allowed);
        assert_eq!(copies, usize::from(allowed));
    }
    let frame = protocol.encode_response(Ok(&value)).unwrap();
    for allowed in [false, true] {
        let mut configured = protocol.clone();
        if !allowed {
            configured.limits.values.string_bytes = 0;
        }
        let (result, copies) = observe(100_000, || configured.decode_response(&frame));
        assert_eq!(result.is_ok(), allowed);
        assert_eq!(copies, usize::from(allowed));
    }
}

#[test]
fn worker_diagnostics_reject_source_bytes_before_copying_source_tables() {
    use botwork::core::{
        ast::Program, diagnostic::Diagnostic, grammar::BWErr, worker::protocol::WorkerProtocol,
    };
    let parsed = Program::parse(&"n".repeat(100_000), "|x| = |1|").unwrap();
    let error = Diagnostic::new(BWErr::NativeError("detail".into())).at(&parsed.statements[0].span);
    let protocol = WorkerProtocol::default();
    let frame = protocol.encode_response(Err(&error)).unwrap();
    for allowed in [false, true] {
        let mut configured = protocol.clone();
        if !allowed {
            configured.limits.diagnostics.source_bytes = 0;
        }
        let (result, copies) = observe(100_000, || configured.decode_response(&frame));
        assert_eq!(result.is_ok(), allowed);
        assert_eq!(copies, usize::from(allowed));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn isolated_operations_reserve_owned_value_and_source_payloads_before_decoding() {
    use botwork::core::{
        ast::Program,
        diagnostic::{Diagnostic, DiagnosticLimits},
        grammar::BWErr,
        operation::{NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits},
        signature::StatementSignature,
        worker::{protocol::WorkerProtocol, WorkerCommand, WorkerLimits, WorkerPool},
    };
    fn command(mode: &str) -> WorkerCommand {
        let executable = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join("python3"))
            .find(|path| path.is_absolute() && path.is_file())
            .unwrap();
        WorkerCommand {
            executable,
            arguments: vec![
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/typed_worker.py")
                    .into_os_string(),
                mode.into(),
            ],
            directory: std::env::temp_dir(),
            environment: Default::default(),
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let pool = WorkerPool::new(WorkerLimits::default()).unwrap();
    for mode in 0..3 {
        let mut operation = NativeOperation::isolated(
            StatementSignature::native("Echo |x|").unwrap(),
            pool.clone(),
            command("large"),
            WorkerProtocol::default(),
        )
        .unwrap();
        if mode == 1 {
            operation = operation
                .with_value_limits(ValueLimits {
                    string_bytes: 0,
                    ..Default::default()
                })
                .unwrap();
        }
        if mode == 2 {
            operation =
                operation.with_ownership_budget(OperationBudget::new(OperationOwnershipLimits {
                    payload_bytes: 0,
                    ..Default::default()
                }));
        }
        let (result, copies) = observe(65536, || {
            runtime.block_on(operation.invoke(vec![Literal::None], OperationControl::default()))
        });
        assert_eq!(result.is_ok(), mode == 0);
        assert_eq!(copies, usize::from(mode == 0));
    }
    let parsed = Program::parse(&"n".repeat(20_000), "|x| = |1|").unwrap();
    let error = Diagnostic::new(BWErr::NativeError("detail".into())).at(&parsed.statements[0].span);
    let frame = WorkerProtocol::default()
        .encode_response(Err(&error))
        .unwrap();
    let mut command = command("echo");
    command.environment.insert(
        "FRAME".into(),
        frame
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .into(),
    );
    for mode in 0..3 {
        let mut operation = NativeOperation::isolated(
            StatementSignature::native("Echo |x|").unwrap(),
            pool.clone(),
            command.clone(),
            WorkerProtocol::default(),
        )
        .unwrap();
        if mode == 1 {
            operation = operation
                .with_diagnostic_limits(DiagnosticLimits {
                    source_bytes: 0,
                    ..Default::default()
                })
                .unwrap();
        }
        if mode == 2 {
            operation =
                operation.with_ownership_budget(OperationBudget::new(OperationOwnershipLimits {
                    source_bytes: 0,
                    ..Default::default()
                }));
        }
        let (result, copies) = observe(20_000, || {
            runtime.block_on(operation.invoke(vec![Literal::None], OperationControl::default()))
        });
        assert_eq!(
            result.unwrap_err().code().as_str(),
            if mode == 0 { "BW4002" } else { "BW8001" }
        );
        // One trusted command/environment copy; source ownership is additional only after admission.
        assert_eq!(copies, 1 + usize::from(mode == 0));
    }
}

#[test]
fn output_rejection_precedes_map_sorting_and_complete_string_buffering() {
    use botwork::core::{eval::Context, run::OutputLimits};
    let value = Literal::Map(
        (0..10_000)
            .map(|index| (format!("key{index:05}"), Literal::None))
            .collect(),
    );
    let threshold = 100_000;
    for record_bytes in [0, 1024 * 1024] {
        let context = Context::with_limits(RunLimits {
            output: OutputLimits {
                record_bytes,
                total_bytes: 1024 * 1024,
            },
            ..Default::default()
        })
        .unwrap();
        let (result, large) = observe(threshold, || {
            context.write_value(&value, &mut std::io::sink())
        });
        assert_eq!(result.is_ok(), record_bytes != 0);
        assert_eq!(large, usize::from(record_bytes != 0)); // Only the admitted map sorting table.
    }
    let value = Literal::String("x".repeat(threshold * 2));
    let context = Context::with_limits(RunLimits {
        output: OutputLimits {
            record_bytes: threshold,
            total_bytes: usize::MAX,
        },
        ..Default::default()
    })
    .unwrap();
    let mut output = Vec::new();
    let (result, large) = observe(threshold, || context.write_value(&value, &mut output));
    assert!(result.is_err());
    assert_eq!(large, 0);
    assert_eq!(output.capacity(), 0);
}

#[test]
fn rejected_rethrow_copy_never_allocates_large_call_metadata() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticCode,
        eval::{evaluate_program_detailed, Context},
        grammar::BWErr,
        run::{RetainedDiagnosticLimits, RunLimits},
    };
    let signature = "A".repeat(64 * 1024);
    let mut counts = Vec::new();
    for (related_locations, body) in [(0, ""), (0, "Rethrow"), (1, "Rethrow")] {
        let mut context = Context::with_limits(RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records: 2, // Active call plus its outgoing error, then original plus rethrow.
                related_locations,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        context
            .register_native(&signature, |_| Err(BWErr::NativeError("failed".into())))
            .unwrap();
        let program =
            Program::parse("copy", &format!("Try {{ {signature} }} Catch {{ {body} }}")).unwrap();
        let (result, allocations) = observe(signature.len(), || {
            evaluate_program_detailed(&program, &mut context)
        });
        match (related_locations, body) {
            (_, "") => {
                result.unwrap();
            }
            (0, _) => assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit),
            _ => assert_eq!(result.unwrap_err().code(), DiagnosticCode::Native),
        }
        counts.push(allocations);
    }
    assert_eq!(counts[1], counts[0]); // Rejection adds no large metadata copy.
    assert_eq!(counts[2], counts[0] + 1); // One admitted rethrow copy, no self-cause copy.
}

#[test]
fn diagnostic_rendering_allocates_one_admitted_output_without_an_intermediate_help_string() {
    use botwork::core::{
        diagnostic::{Diagnostic, DiagnosticRenderLimits},
        grammar::BWErr,
    };
    let name = "é".repeat(16 * 1024);
    let error = Diagnostic::new(BWErr::undefined_variable(name.clone()));
    let (rendered, copies) = observe(16 * 1024, || {
        error.render_with_limits(&DiagnosticRenderLimits {
            output_bytes: 128 * 1024,
            ..Default::default()
        })
    });
    assert!(rendered.truncation.is_none());
    assert_eq!(rendered.text.matches(&name).count(), 2); // Message and repair guidance.
    assert_eq!(copies, 1); // The single complete output buffer.
}

#[test]
fn rejected_diagnostic_rendering_never_copies_large_filename_detail_or_help_buffers() {
    use botwork::core::{
        ast::Program,
        diagnostic::{Diagnostic, RENDER_SUMMARY_BYTES},
        grammar::BWErr,
    };
    for filename in [false, true] {
        let name = if filename {
            "é".repeat(100_000)
        } else {
            "source".into()
        };
        let program = Program::parse(&name, "|x| = |1|").unwrap();
        let error = Diagnostic::new(if filename {
            BWErr::NativeError("failed".into())
        } else {
            BWErr::undefined_variable("🙂".repeat(100_000))
        })
        .at(&program.statements[0].span);
        let (rendered, copies) = observe(64 * 1024, || error.to_string());
        assert!(rendered.len() <= RENDER_SUMMARY_BYTES);
        assert!(rendered.contains("diagnostic rendering truncated"));
        assert_eq!(copies, 0);
        if !filename {
            let (help, copies) = observe(64 * 1024, || error.help());
            assert!(help.contains("repair guidance truncated"));
            assert!(help.len() <= RENDER_SUMMARY_BYTES);
            assert_eq!(copies, 0);
        }
    }
}

#[test]
fn operation_ownership_admission_moves_values_and_rejects_errors_without_large_copies() {
    use botwork::core::{
        diagnostic::{Diagnostic, DiagnosticCode},
        grammar::BWErr,
        operation::{
            NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits,
            OperationUsage,
        },
        signature::StatementSignature,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let budget = OperationBudget::default();
    let operation = NativeOperation::asynchronous(
        StatementSignature::native("Echo |value|").unwrap(),
        |mut values, _| async move { Ok(values.pop().unwrap()) },
    )
    .unwrap()
    .with_ownership_budget(budget.clone());
    let text = "é".repeat(64 * 1024);
    let pointer = text.as_ptr();
    let (value, copies) = observe(64 * 1024, || {
        runtime
            .block_on(operation.invoke(vec![Literal::String(text)], OperationControl::default()))
            .unwrap()
    });
    assert_eq!(copies, 0);
    let Literal::String(text) = value else {
        panic!("string")
    };
    assert_eq!(text.as_ptr(), pointer);
    assert_eq!(budget.usage(), OperationUsage::default());
    let error = Mutex::new(Some(Diagnostic::new(BWErr::NativeError(
        "é".repeat(64 * 1024),
    ))));
    let budget = OperationBudget::new(OperationOwnershipLimits {
        text_bytes: 0,
        ..OperationOwnershipLimits::default()
    });
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Fail").unwrap(), move |_, _| {
            let error = error.lock().unwrap().take().unwrap();
            async move { Err(error) }
        })
        .unwrap()
        .with_ownership_budget(budget.clone());
    let (error, copies) = observe(64 * 1024, || {
        runtime
            .block_on(operation.invoke(vec![], OperationControl::default()))
            .unwrap_err()
    });
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Native);
    assert_eq!(copies, 0);
    assert_eq!(budget.usage(), OperationUsage::default());
}

#[test]
fn file_input_errors_stream_large_origins_without_extra_path_sized_buffers() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        input::{load_variables_with_limits, InputLimits},
    };
    use std::path::PathBuf;
    let check_path = |path: PathBuf| {
        for source_limit in [0, 1] {
            // Platform open arguments have their own ownership. Removing the
            // origin copy must not hide those required allocations in the comparison.
            let platform_copies = if source_limit == 0 {
                0
            } else {
                let (result, copies) = observe(64 * 1024, || std::fs::File::open(&path));
                assert!(result.is_err());
                copies
            };
            let (result, copies) = observe(64 * 1024, || {
                load_variables_with_limits(
                    std::slice::from_ref(&path),
                    &[],
                    &InputLimits {
                        sources: source_limit,
                        ..InputLimits::default()
                    },
                )
            });
            let error = result.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(
                error.causes[0].code(),
                if source_limit == 0 {
                    DiagnosticCode::ResourceLimit
                } else {
                    DiagnosticCode::Input
                }
            );
            assert_eq!(copies, platform_copies, "source limit {source_limit}");
        }
    };
    check_path(PathBuf::from(
        "é".repeat(DiagnosticLimits::default().source_bytes),
    ));
    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        check_path(PathBuf::from(OsString::from_vec(
            vec![0xff; DiagnosticLimits::default().source_bytes],
        )));
    }
}

#[test]
fn worker_join_construction_allocates_only_admitted_full_messages_and_cleanup_causes() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        operation::{NativeOperation, OperationControl},
        signature::StatementSignature,
    };
    use std::{
        future::Future,
        num::NonZeroUsize,
        task::{Context, Waker},
        time::Duration,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    for cleanup in [false, true] {
        for reject in [false, true] {
            let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            let operation = NativeOperation::blocking(
                StatementSignature::native("Read").unwrap(),
                NonZeroUsize::new(1).unwrap(),
                move |_, _| {
                    if cleanup {
                        started_tx.send(()).unwrap();
                        release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap();
                    }
                    std::panic::panic_any(worker_panic::EscapingPanic(Some("é\n".repeat(32_768))));
                },
            )
            .unwrap()
            .with_diagnostic_limits(if reject {
                if cleanup {
                    // The worker error alone fits; its known stop root does not.
                    DiagnosticLimits {
                        diagnostics: 1,
                        ..DiagnosticLimits::default()
                    }
                } else {
                    DiagnosticLimits {
                        text_bytes: 32,
                        ..DiagnosticLimits::default()
                    }
                }
            } else {
                DiagnosticLimits::default()
            })
            .unwrap();
            // Only the invoking thread's allocations are observed. Worker-owned
            // panic payloads and Tokio's panic handling are a separate boundary.
            let (error, copies) = observe(64 * 1024, || {
                runtime.block_on(async {
                    let control = OperationControl::default();
                    let mut invocation = Box::pin(operation.invoke(vec![], control.clone()));
                    if cleanup {
                        assert!(invocation
                            .as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                            .is_pending());
                        started_rx.recv().await.unwrap();
                        control.cancel();
                        assert!(invocation
                            .as_mut()
                            .poll(&mut Context::from_waker(Waker::noop()))
                            .is_pending());
                        release_tx.send(()).unwrap();
                    }
                    invocation.await.unwrap_err()
                })
            });
            assert_eq!(
                copies,
                usize::from(!reject),
                "cleanup={cleanup}, reject={reject}"
            );
            assert_eq!(
                error.code(),
                if cleanup {
                    DiagnosticCode::Cancelled
                } else if reject {
                    DiagnosticCode::ResourceLimit
                } else {
                    DiagnosticCode::AsyncRuntime
                }
            );
            if cleanup {
                assert_eq!(
                    error.causes[0].code(),
                    if reject {
                        DiagnosticCode::ResourceLimit
                    } else {
                        DiagnosticCode::AsyncRuntime
                    }
                );
            }
        }
    }
}

#[test]
fn guard_rejection_copies_neither_large_filenames_nor_checked_source_prefixes() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        syntax_limits::SyntaxLimits,
    };
    let filename = "é".repeat(128 * 1024);
    let engine = Engine::default();
    for (source, source_bytes, syntax) in [
        (
            format!("{}Log |1 + 2|", " ".repeat(128 * 1024)),
            1024 * 1024,
            SyntaxLimits {
                nesting: 32,
                operators: 0,
            },
        ),
        (
            format!("{}Log |[[1]]|", " ".repeat(128 * 1024)),
            1024 * 1024,
            SyntaxLimits {
                nesting: 2,
                operators: 64,
            },
        ),
    ] {
        for reject in [false, true] {
            let (run, copies) = observe(64 * 1024, || {
                engine.run_source(
                    &filename,
                    &source,
                    RunOptions {
                        limits: RunLimits {
                            source_bytes,
                            syntax: syntax.clone(),
                            diagnostics: if reject {
                                DiagnosticLimits {
                                    source_bytes: 0,
                                    ..DiagnosticLimits::default()
                                }
                            } else {
                                DiagnosticLimits::default()
                            },
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                )
            });
            let error = run.result.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            if reject {
                assert!(error.causes[0].span.is_none());
                assert!(
                    error.causes[0]
                        .omissions
                        .as_ref()
                        .unwrap()
                        .source
                        .as_ref()
                        .unwrap()
                        .file_truncated
                );
                assert_eq!(copies, 0);
            } else {
                assert!(error.span.as_ref().unwrap().source().text().len() >= 128 * 1024);
                assert!(error.causes.is_empty());
                assert_eq!(copies, 2); // Admitted filename and checked prefix only.
            }
        }
    }
}

#[test]
fn standalone_source_guard_rejection_precedes_default_filename_and_prefix_copies() {
    use botwork::core::{ast::Program, diagnostic::DiagnosticLimits, syntax_limits::SyntaxLimits};
    let source = format!("{}🦀", "x".repeat(128 * 1024));
    for reject in [false, true] {
        let filename = "n"
            .repeat(DiagnosticLimits::default().source_bytes - source.len() + usize::from(reject));
        let (result, copies) = observe(64 * 1024, || {
            Program::parse_bounded(&filename, &source, 128 * 1024 + 2, &SyntaxLimits::default())
        });
        let error = result.unwrap_err();
        if reject {
            assert!(error.causes[0].span.is_none());
            assert_eq!(copies, 0);
            let evidence = error.causes[0]
                .omissions
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap();
            assert_eq!(
                (evidence.start_byte, evidence.end_byte),
                (128 * 1024, source.len())
            );
        } else {
            assert!(error.span.is_some());
            assert_eq!(copies, 2);
        }
    }
}

#[test]
fn syntax_construction_allocates_only_the_admitted_final_message_beyond_parser_owned_buffers() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        grammar::BWParser,
    };
    use pest::Parser;
    let source = format!("{}|x| = |1 +|", " \t".repeat(128 * 1024));
    let (parser, parsing_copies) = observe(64 * 1024, || BWParser::parse(Rule::botwork, &source));
    let parser = parser.unwrap_err();
    let bytes = "source".len() + parser.to_string().len();
    let engine = Engine::default();
    for reject in [false, true] {
        let (run, copies) = observe(64 * 1024, || {
            engine.run_source(
                "syntax",
                &source,
                RunOptions {
                    limits: RunLimits {
                        diagnostics: DiagnosticLimits {
                            text_bytes: bytes - usize::from(reject),
                            ..DiagnosticLimits::default()
                        },
                        ..RunLimits::default()
                    },
                    ..RunOptions::default()
                },
            )
        });
        let error = run.result.unwrap_err();
        if reject {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::Syntax);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        } else {
            assert_eq!(error.code(), DiagnosticCode::Syntax);
        }
        // One interpreter source owner plus one final message only when admitted.
        assert_eq!(copies, parsing_copies + 1 + usize::from(!reject));
    }
}

#[test]
fn validation_limits_skip_large_location_copies_while_preserving_required_source_ownership() {
    use botwork::core::diagnostic::{DiagnosticCode, DiagnosticLimits};
    let filename = "é".repeat(128 * 1024);
    let engine = Engine::default();
    for (source, code, location_fields) in [
        ("Return", DiagnosticCode::InvalidControl, 1),
        (
            "Read |x| with |x| {}",
            DiagnosticCode::DuplicateParameter,
            2,
        ),
    ] {
        let (accepted, full_copies) = observe(64 * 1024, || {
            engine.run_source(&filename, source, RunOptions::default())
        });
        assert_eq!(accepted.result.unwrap_err().code(), code);
        for diagnostics in [
            DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            DiagnosticLimits {
                source_bytes: 0,
                ..DiagnosticLimits::default()
            },
        ] {
            let (rejected, copies) = observe(64 * 1024, || {
                engine.run_source(
                    &filename,
                    source,
                    RunOptions {
                        limits: RunLimits {
                            diagnostics,
                            ..RunLimits::default()
                        },
                        ..RunOptions::default()
                    },
                )
            });
            let error = rejected.result.unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), code);
            assert_eq!(
                error.causes[0].omissions.as_ref().unwrap().detail_fields,
                location_fields
            );
            assert_eq!(copies, 1); // The parser still owns its source filename.
            assert_eq!(full_copies, copies + location_fields);
        }
    }
}

#[test]
fn collision_errors_reject_large_source_names_without_location_message_copies() {
    use botwork::core::{
        ast::Program,
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::{evaluate_program_detailed, Context},
    };
    let filename = "é".repeat(128 * 1024);
    let original = Program::parse(&filename, "Read { Return |7| }").unwrap();
    let duplicate = Program::parse(&filename, "Read {}").unwrap();
    for native in [false, true] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                source_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        if native {
            context
                .register_native("Read", |_| Ok(Literal::Int(7)))
                .unwrap();
        } else {
            evaluate_program_detailed(&original, &mut context).unwrap();
        }
        let mut sibling = context.clone();
        let (result, large) = observe(64 * 1024, || {
            evaluate_program_detailed(&duplicate, &mut context)
        });
        let error = result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::DuplicateStatement);
        let omitted = error.causes[0].omissions.as_ref().unwrap();
        assert_eq!(omitted.detail_fields, if native { 1 } else { 2 });
        assert!(omitted.source.as_ref().unwrap().file_truncated);
        assert_eq!(large, 0);
        let call = Program::parse("call", "Read").unwrap();
        assert_eq!(
            evaluate_program_detailed(&call, &mut sibling)
                .unwrap()
                .to_string(),
            "7"
        );
    }
}

#[test]
fn native_collision_rejects_a_large_signature_before_error_detail_allocation() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::Context,
        signature::StatementSignature,
    };
    let name = "R".repeat(64 * 1024);
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native(&name, |_| Ok(Literal::Int(1)))
        .unwrap();
    let duplicate = StatementSignature::native(&name).unwrap();
    let (result, large) = observe(name.len(), || {
        context.register_native_with_signature(duplicate, |_| Ok(Literal::Int(2)))
    });
    let error = result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::DuplicateStatement);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 2);
    assert_eq!(large, 0);
}

#[test]
fn setup_message_rejection_adds_no_large_copies_and_prior_stops_skip_path_preparation() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        operation::OperationControl,
    };
    let length = DiagnosticLimits::default().text_bytes;
    let path = std::path::PathBuf::from("x".repeat(length));
    let (_, required_copies) = observe(length, || {
        let directory = path.clone();
        std::fs::canonicalize(&directory).unwrap_err()
    });
    let engine = Engine::default();
    for stopped in 0..3 {
        let control = OperationControl::default();
        if stopped == 1 {
            control.cancel();
        }
        let options = RunOptions {
            working_directory: Some(path.clone()),
            inherit_environment: false,
            control,
            timeout: (stopped == 2).then_some(std::time::Duration::ZERO),
            ..RunOptions::default()
        };
        let (run, large) = observe(length, || engine.run_source("setup", "Unknown", options));
        let error = run.result.unwrap_err();
        if stopped == 0 {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::RunConfiguration);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
            assert_eq!(large, required_copies);
        } else {
            assert_eq!(
                error.code(),
                if stopped == 1 {
                    DiagnosticCode::Cancelled
                } else {
                    DiagnosticCode::Timeout
                }
            );
            assert_eq!(large, 0);
        }
    }
}

#[test]
fn import_messages_reserve_originating_site_before_copying_a_large_invalid_path() {
    use botwork::core::{
        ast::Program,
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::{evaluate_program_detailed, Context},
    };
    let path = "x".repeat(128 * 1024);
    let message_bytes = path.len() + "`` must name a local .botwork file".len();
    let program = Program::parse("import", &format!("Import |\"{path}\"| As |lib|")).unwrap();
    // The second quota would fit the full message and label if the originating
    // import site were incorrectly appended only after message construction.
    for text_bytes in [0, message_bytes + "source".len()] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let (result, large) = observe(64 * 1024, || {
            evaluate_program_detailed(&program, &mut context)
        });
        let error = result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
        let omitted = error.causes[0].omissions.as_ref().unwrap();
        assert_eq!(omitted.related_locations, 1);
        assert_eq!(omitted.detail_fields, 1);
        assert_eq!(large, 0);
    }
}

#[cfg(unix)]
#[test]
fn import_cycle_rejection_formats_no_large_joined_path_chain() {
    use botwork::core::diagnostic::{DiagnosticCode, DiagnosticLimits};
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let cleanup = Cleanup(std::env::temp_dir().join(format!(
            "botwork-cycle-allocation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    let mut directory = cleanup.0.clone();
    // Keep each filesystem path below typical Unix path limits while the
    // complete 13-member diagnostic chain still exceeds the observed threshold.
    // The 57-entry fixed statement table now exceeds 16 KiB; use longer paths
    // and a 32 KiB threshold to keep measuring diagnostic payload allocations.
    for _ in 0..24 {
        directory.push("x".repeat(120));
    }
    std::fs::create_dir_all(&directory).unwrap();
    for index in 0..12 {
        std::fs::write(
            directory.join(format!("module{index}.botwork")),
            format!("Import |\"module{}.botwork\"| As |next|", (index + 1) % 12),
        )
        .unwrap();
    }
    let engine = Engine::default();
    for text_bytes in [0, DiagnosticLimits::default().text_bytes] {
        let options = RunOptions {
            working_directory: Some(directory.clone()),
            inherit_environment: false,
            limits: RunLimits {
                diagnostics: DiagnosticLimits {
                    text_bytes,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        };
        let (run, large) = observe(32 * 1024, || {
            engine.run_source("entry", "Import |\"module0.botwork\"| As |lib|", options)
        });
        let error = run.result.unwrap_err();
        if text_bytes == 0 {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::ImportCycle);
            assert_eq!(
                error.causes[0]
                    .omissions
                    .as_ref()
                    .unwrap()
                    .related_locations,
                13
            );
            assert_eq!(large, 0);
        } else {
            assert_eq!(error.code(), DiagnosticCode::ImportCycle);
            assert_eq!(error.related.len(), 13);
            assert_eq!(large, 1); // The single admitted final chain; no intermediate join.
        }
    }
}

#[test]
fn entry_file_read_errors_admit_detail_before_copying_the_large_requested_path() {
    use botwork::core::diagnostic::{DiagnosticCode, DiagnosticLimits};
    let path = "x".repeat(64 * 1024);
    let engine = Engine::default();
    let run = |text_bytes| {
        engine.run_file(
            &path,
            RunOptions {
                inherit_environment: false,
                limits: RunLimits {
                    diagnostics: DiagnosticLimits {
                        text_bytes,
                        ..DiagnosticLimits::default()
                    },
                    ..RunLimits::default()
                },
                ..RunOptions::default()
            },
        )
    };
    let (ordinary, full_copies) =
        observe(path.len(), || run(DiagnosticLimits::default().text_bytes));
    assert_eq!(
        ordinary.result.unwrap_err().code(),
        DiagnosticCode::SourceRead
    );
    let (rejected, limited_copies) = observe(path.len(), || run(0));
    let error = rejected.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::SourceRead);
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
    // Path joining and the platform file-open argument still own their required
    // path copies. Rejection removes the single additional diagnostic message.
    assert_eq!(full_copies, limited_copies + 1);
}

#[test]
fn signature_builder_rejects_large_unknown_parameter_messages_before_owned_copies() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        signature::{StatementSignature, ValueKind},
    };
    let name = "é".repeat(DiagnosticLimits::default().text_bytes / 2);
    let signature = StatementSignature::native("Read |value|").unwrap();
    let (result, large) = observe(64 * 1024, || signature.parameter(&name, ValueKind::Int));
    let error = result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Signature);
    let omitted = error.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omitted.detail_fields, 1);
    assert_eq!(omitted.source.as_ref().unwrap().file, "<native>");
    assert_eq!(large, 0);
}

#[test]
fn input_conversion_and_resource_errors_admit_large_origins_before_message_or_source_copies() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        input::{parse_variable, parse_variables, parse_variables_with_limits, InputLimits},
    };
    let origin = "é".repeat(DiagnosticLimits::default().source_bytes / 2 + 1);
    for branch in 0..4 {
        let (result, large) = observe(64 * 1024, || match branch {
            0 => parse_variable(&origin, "no_equals").map(|_| ()),
            1 => parse_variables(&origin, r#"{"x":1e9999}"#).map(|_| ()),
            2 => parse_variables(&origin, r#"{"x":"#).map(|_| ()),
            _ => parse_variables_with_limits(
                &origin,
                "{}",
                &InputLimits {
                    sources: 0,
                    ..InputLimits::default()
                },
            )
            .map(|_| ()),
        });
        let error = result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(large, 0);
        if branch == 3 {
            assert_eq!(error.causes[0].code(), DiagnosticCode::ResourceLimit);
            let omitted = error.causes[0].omissions.as_ref().unwrap();
            assert!(omitted.source.as_ref().unwrap().file_truncated);
            assert_eq!(omitted.detail_fields, 0);
        } else {
            assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        }
    }
}

#[test]
fn standalone_invalid_input_names_reject_large_origins_before_detail_allocation() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        input::{parse_variable, parse_variables},
    };
    let origin = "é".repeat(DiagnosticLimits::default().text_bytes / 2);
    for from_json in [false, true] {
        let (result, large) = observe(64 * 1024, || {
            if from_json {
                parse_variables(&origin, r#"{"!":null}"#).map(|_| ())
            } else {
                parse_variable(&origin, "!=null").map(|_| ())
            }
        });
        let error = result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        assert_eq!(large, 0);
    }
}

#[test]
fn invalid_host_names_and_nonfinite_value_messages_reject_without_large_parser_or_detail_copies() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::Context,
    };
    let length = 64 * 1024;
    for invalid_name in [false, true] {
        let name = if invalid_name {
            format!("!{}", "x".repeat(length - 1))
        } else {
            "x".repeat(length)
        };
        let value = if invalid_name {
            Literal::None
        } else {
            Literal::Float(f32::INFINITY)
        };
        let variables = BTreeMap::from([(name, value)]);
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 32,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let (result, large) = observe(length, || context.set_input_variables(variables));
        let error = result.unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        assert_eq!(error.causes[0].code(), DiagnosticCode::Input);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        assert_eq!(large, 0);
    }
}

#[test]
fn incompatible_operator_messages_reject_large_debug_details_before_allocation() {
    use botwork::core::{
        ast::Program,
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    for expression in ["value - 1", "-value", "value and true"] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 32,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        context
            .set_input_variables(BTreeMap::from([(
                "value".into(),
                Literal::String("x".repeat(length)),
            )]))
            .unwrap();
        let program = Program::parse("operator", &format!("|out| = |{expression}|")).unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        let error = result.unwrap_err();
        assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
        assert!(error.causes[0].omissions.is_some());
        assert_eq!(large, 1); // required value snapshot; no large diagnostic detail buffer
    }
}

#[test]
fn legacy_operator_default_diagnostic_budget_bounds_escaped_debug_expansion() {
    use botwork::core::grammar::BWErr;
    let length = 1024 * 1024;
    let left = Literal::String("\u{1b}".repeat(length));
    let right = Literal::String("\u{1b}".repeat(length));
    let (result, large) = observe(64 * 1024, || Rule::minus.operate_binary(left, right));
    assert!(matches!(
        result,
        Err(BWErr::ResourceLimit {
            resource: "diagnostic text bytes",
            limit: 8_388_608
        })
    ));
    assert_eq!(large, 0);
}

#[test]
fn access_error_admission_rejects_the_complete_group_before_copying_large_path_or_segment() {
    use botwork::core::{
        ast::Program,
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    let name = "x".repeat(length);
    for computed in [false, true] {
        let suffix = if computed {
            format!("[\"{name}\"]")
        } else {
            format!(".{name}")
        };
        let program = Program::parse(
            "access",
            &format!("|data| = |{{}}|\n|out| = |data{suffix}|"),
        )
        .unwrap();
        for text_bytes in [32, length + 64] {
            let mut context = Context::with_limits(RunLimits {
                diagnostics: DiagnosticLimits {
                    text_bytes,
                    ..DiagnosticLimits::default()
                },
                ..RunLimits::default()
            })
            .unwrap();
            let (result, large) =
                observe(length, || evaluate_program_detailed(&program, &mut context));
            let error = result.unwrap_err();
            assert_eq!(error.causes[0].code(), DiagnosticCode::CollectionAccess);
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 2);
            // Computed keys retain their required string-value construction; diagnostics add no large copies.
            assert_eq!(large, usize::from(computed));
        }
    }
}

#[test]
fn synchronous_signature_failures_reject_large_parameter_and_return_messages_before_allocation() {
    use botwork::core::{
        ast::Program,
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::{evaluate_program_detailed, Context},
        signature::{StatementSignature, ValueKind},
    };
    let length = 64 * 1024;
    let name = "x".repeat(length);
    for returning in [false, true] {
        let header = if returning {
            name.clone()
        } else {
            format!("Read |{name}|")
        };
        let signature = StatementSignature::native(&header).unwrap();
        let signature = if returning {
            signature.returns(ValueKind::String)
        } else {
            signature.parameter(&name, ValueKind::Int).unwrap()
        };
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 32,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        context
            .register_native_with_signature(signature, move |_| {
                assert!(returning, "rejected callee entered");
                Ok(Literal::Bool(true))
            })
            .unwrap();
        let program =
            Program::parse("formatted", if returning { &name } else { "Read |true|" }).unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        let error = result.unwrap_err();
        assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        assert_eq!(large, usize::from(returning));
    }
}

#[test]
fn operation_signature_failures_reject_large_parameter_and_return_messages_before_allocation() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        operation::{NativeOperation, OperationControl},
        signature::{StatementSignature, ValueKind},
    };
    let length = 64 * 1024;
    let name = "x".repeat(length);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    for returning in [false, true] {
        let header = if returning {
            name.clone()
        } else {
            format!("Read |{name}|")
        };
        let signature = StatementSignature::native(&header).unwrap();
        let signature = if returning {
            signature.returns(ValueKind::String)
        } else {
            signature.parameter(&name, ValueKind::Int).unwrap()
        };
        let operation = NativeOperation::asynchronous(signature, move |_, _| {
            assert!(returning, "rejected factory entered");
            async { Ok(Literal::Bool(true)) }
        })
        .unwrap()
        .with_diagnostic_limits(DiagnosticLimits {
            text_bytes: 32,
            ..DiagnosticLimits::default()
        })
        .unwrap();
        let arguments = if returning {
            vec![]
        } else {
            vec![Literal::Bool(true)]
        };
        let (result, large) = observe(length, || {
            runtime.block_on(operation.invoke(arguments, OperationControl::default()))
        });
        let error = result.unwrap_err();
        assert_eq!(error.causes[0].code(), DiagnosticCode::IncompatibleType);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        assert_eq!(large, 0);
    }
}

#[test]
fn operation_factory_and_poll_panics_do_not_copy_rejected_large_signatures() {
    use botwork::core::{
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        operation::{NativeOperation, OperationControl},
        signature::StatementSignature,
    };
    let length = 64 * 1024;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    for factory in [false, true] {
        let signature = StatementSignature::native(&"x".repeat(length)).unwrap();
        let operation = NativeOperation::asynchronous(signature, move |_, _| {
            assert!(!factory, "factory panic");
            async { panic!("poll panic") }
        })
        .unwrap()
        .with_diagnostic_limits(DiagnosticLimits {
            text_bytes: 32,
            ..DiagnosticLimits::default()
        })
        .unwrap();
        let (result, large) = observe(length, || {
            runtime.block_on(operation.invoke(vec![], OperationControl::default()))
        });
        let error = result.unwrap_err();
        assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        assert!(!error.causes[0].omissions.as_ref().unwrap().prior_summary);
        assert_eq!(large, 0);
    }
}

#[test]
fn runtime_name_and_panic_construction_rejects_before_copying_borrowed_details() {
    use botwork::core::{
        ast::Program,
        diagnostic::{DiagnosticCode, DiagnosticLimits},
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    let name = "x".repeat(length);
    for (source, native, category) in [
        (
            format!("|out| = |{name}|"),
            false,
            DiagnosticCode::UndefinedVariable,
        ),
        (
            format!("|out| = |{name}.field|"),
            false,
            DiagnosticCode::UndefinedVariable,
        ),
        (name.clone(), false, DiagnosticCode::UndefinedStatement),
        (name.clone(), true, DiagnosticCode::NativePanic),
    ] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 32,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        if native {
            context
                .register_native(&name, |_| panic!("callback"))
                .unwrap();
        }
        let program = Program::parse("construction", &source).unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        let error = result.unwrap_err();
        assert_eq!(error.causes[0].code(), category);
        assert_eq!(error.causes[0].omissions.as_ref().unwrap().detail_fields, 1);
        assert!(error.causes[0].omissions.as_ref().unwrap().source.is_some());
        // The accepted active native call still owns one signature copy.
        assert_eq!(large, usize::from(native));
    }
}

#[test]
fn operation_rejects_large_host_error_details_without_copying_them() {
    use botwork::core::{
        diagnostic::{Diagnostic, DiagnosticLimits},
        grammar::BWErr,
        operation::{NativeOperation, OperationControl},
        signature::StatementSignature,
    };
    let length = 64 * 1024;
    let error = Mutex::new(Some(Diagnostic::new(BWErr::NativeError(
        "x".repeat(length),
    ))));
    let operation =
        NativeOperation::asynchronous(StatementSignature::native("Fail").unwrap(), move |_, _| {
            let error = error.lock().unwrap().take().unwrap();
            async move { Err(error) }
        })
        .unwrap()
        .with_diagnostic_limits(DiagnosticLimits {
            text_bytes: 64,
            ..DiagnosticLimits::default()
        })
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let (result, large) = observe(length, || {
        runtime.block_on(operation.invoke(vec![], OperationControl::default()))
    });
    assert_eq!(
        result.unwrap_err().causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .detail_fields,
        1
    );
    assert_eq!(large, 0);
}

#[test]
fn active_call_retention_rejects_before_copying_a_large_signature() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticCode,
        eval::{evaluate_program_detailed, Context},
        run::RetainedDiagnosticLimits,
    };
    let length = 64 * 1024;
    let header = "x".repeat(length);
    for records in [0, 1] {
        let mut context = Context::with_limits(RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records,
                text_bytes: 0,
                ..RetainedDiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        context
            .register_native(&header, |_| panic!("rejected call entered"))
            .unwrap();
        let program = Program::parse("runtime", &header).unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
        assert_eq!(large, 0);
    }
}

#[test]
fn handler_retention_rejects_before_copying_native_details_into_catch_metadata() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        grammar::BWErr,
        run::RetainedDiagnosticLimits,
    };
    let length = 64 * 1024;
    let reason = Mutex::new(Some("x".repeat(length)));
    let mut context = Context::with_limits(RunLimits {
        retained_diagnostics: RetainedDiagnosticLimits {
            text_bytes: 4,
            ..RetainedDiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native("Fail", move |_| {
            Err(BWErr::NativeError(reason.lock().unwrap().take().unwrap()))
        })
        .unwrap();
    let program = Program::parse("runtime", "Try { Fail } Catch |error| {}").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert_eq!(
        result.unwrap_err().causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .detail_fields,
        1
    );
    assert_eq!(large, 0);
}

#[test]
fn runtime_diagnostic_limit_precedes_native_reason_copy_and_preserves_small_evidence() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticLimits,
        eval::{evaluate_program_detailed, Context},
        grammar::BWErr,
    };
    let length = 64 * 1024;
    let reason = Mutex::new(Some("x".repeat(length)));
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 32,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native("Fail", move |_| {
            Err(BWErr::NativeError(reason.lock().unwrap().take().unwrap()))
        })
        .unwrap();
    let program = Program::parse("runtime", "Fail").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    let error = result.unwrap_err();
    assert!(error.causes[0].omissions.is_some());
    assert_eq!(large, 0);
}

#[test]
fn runtime_diagnostic_rejection_does_not_copy_large_entered_call_signatures() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticLimits,
        eval::{evaluate_program_detailed, Context},
        grammar::BWErr,
    };
    let length = 64 * 1024;
    let header = "x".repeat(length);
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 32,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native(&header, |_| Err(BWErr::NativeError("reason".into())))
        .unwrap();
    let program = Program::parse("runtime", &header).unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    let error = result.unwrap_err();
    assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
    // The active call frame owns its signature; no diagnostic snapshot repeats it.
    assert_eq!(large, 1);
}

#[test]
fn owned_diagnostic_rejection_does_not_copy_large_details_or_filenames() {
    use botwork::core::{
        ast::Program,
        diagnostic::{Diagnostic, DiagnosticLimits},
        grammar::BWErr,
    };
    let length = 64 * 1024;
    let program = Program::parse(&"f".repeat(length), "|x| = |1|").unwrap();
    let error =
        Diagnostic::new(BWErr::NativeError("x".repeat(length))).at(&program.statements[0].span);
    let (result, large) = observe(length, || {
        DiagnosticLimits {
            text_bytes: 0,
            ..DiagnosticLimits::default()
        }
        .admit(error)
    });
    let rejected = result.unwrap_err();
    assert!(rejected.causes[0].omissions.is_some());
    assert_eq!(large, 0);
}

#[test]
fn checked_diagnostic_clone_rejects_large_context_and_width_before_copying() {
    use botwork::core::{
        ast::Program,
        diagnostic::{CallFrame, Diagnostic, DiagnosticLimits},
        grammar::BWErr,
    };
    let length = 64 * 1024;
    let program = Program::parse("context", "|x| = |1|").unwrap();
    let mut diagnostic = Diagnostic::new(BWErr::NativeError("".into()));
    diagnostic.call_stack.push(CallFrame {
        signature: "x".repeat(length),
        statement: None,
        call_site: program.statements[0].span.clone(),
        definition_site: None,
    });
    let limits = DiagnosticLimits {
        text_bytes: 128,
        ..DiagnosticLimits::default()
    };
    let (result, large) = observe(length, || diagnostic.try_clone_with_limits(&limits));
    assert!(result.is_err());
    assert_eq!(large, 0);
    diagnostic.call_stack.clear();
    diagnostic.causes = (0..4096)
        .map(|_| Diagnostic::new(BWErr::NativeError("".into())))
        .collect();
    let (result, large) = observe(4096, || {
        diagnostic.try_clone_with_limits(&DiagnosticLimits::default())
    });
    assert!(result.is_err());
    assert_eq!(large, 0);
    diagnostic.discard();
}

#[test]
fn diagnostic_message_and_help_are_rejected_before_owned_string_formatting() {
    use botwork::core::{
        diagnostic::{Diagnostic, DiagnosticValueLimits},
        grammar::BWErr,
    };
    let length = 64 * 1024;
    for diagnostic in [
        Diagnostic::new(BWErr::NativeError("x".repeat(length))),
        Diagnostic::new(BWErr::undefined_variable("x".repeat(length))),
    ] {
        let limits = DiagnosticValueLimits {
            values: ValueLimits {
                string_bytes: 128,
                ..ValueLimits::default()
            },
            ..DiagnosticValueLimits::default()
        };
        let (result, large) = observe(length, || diagnostic.to_value_with_limits(&limits));
        assert!(result.is_err());
        assert_eq!(large, 0);
    }
}

#[test]
fn wide_diagnostic_metadata_rejects_before_allocating_array_or_traversal_storage() {
    use botwork::core::{
        diagnostic::{Diagnostic, DiagnosticValueLimits},
        grammar::BWErr,
    };
    let mut diagnostic = Diagnostic::new(BWErr::NativeError("reason".into()));
    diagnostic.causes = (0..4096)
        .map(|_| Diagnostic::new(BWErr::NativeError("reason".into())))
        .collect();
    let limits = DiagnosticValueLimits {
        values: ValueLimits {
            entries: 8,
            ..ValueLimits::default()
        },
        ..DiagnosticValueLimits::default()
    };
    let (result, large) = observe(4096, || diagnostic.to_value_with_limits(&limits));
    assert!(result.is_err());
    assert_eq!(large, 0);
}

#[test]
fn catch_temporary_admission_precedes_diagnostic_payload_copies() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        grammar::BWErr,
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    let reason = Mutex::new(Some("x".repeat(length)));
    let mut context = Context::with_limits(RunLimits {
        temporaries: TemporaryLimits {
            payload_bytes: 0,
            ..TemporaryLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .register_native("Fail", move |_| {
            Err(BWErr::NativeError(reason.lock().unwrap().take().unwrap()))
        })
        .unwrap();
    let program = Program::parse("catch", "Try { Fail } Catch |error| {}").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    let error = result.unwrap_err();
    assert!(matches!(
        error.error.as_ref(),
        BWErr::ResourceLimit {
            resource: "temporary value payload bytes",
            ..
        }
    ));
    assert_eq!(error.causes.len(), 1);
    assert_eq!(large, 0);
}

#[test]
fn temporary_variable_copy_is_admitted_before_payload_allocation() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        temporaries: TemporaryLimits {
            payload_bytes: 0,
            ..TemporaryLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .set_input_variables(BTreeMap::from([(
            "source".into(),
            Literal::String("x".repeat(length)),
        )]))
        .unwrap();
    let program = Program::parse("copy", "|target| = |source|").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("temporary value payload bytes"));
    assert_eq!(large, 0);
}

#[test]
fn temporary_array_width_and_map_keys_reject_before_container_or_key_allocation() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let source = format!("|x| = |[{}]|", vec!["0"; 4096].join(","));
    let array = Program::parse("array", &source).unwrap();
    let map = Program::parse(
        "map",
        &format!("|x| = |{{\"{}\":0}}|", "x".repeat(64 * 1024)),
    )
    .unwrap();
    for (program, limits, threshold, resource) in [
        (
            &array,
            TemporaryLimits {
                nodes: 1,
                ..TemporaryLimits::default()
            },
            16 * 1024,
            "temporary value nodes",
        ),
        (
            &map,
            TemporaryLimits {
                payload_bytes: 0,
                ..TemporaryLimits::default()
            },
            64 * 1024,
            "temporary value payload bytes",
        ),
    ] {
        let mut context = Context::with_limits(RunLimits {
            temporaries: limits,
            ..RunLimits::default()
        })
        .unwrap();
        let (result, large) = observe(threshold, || {
            evaluate_program_detailed(program, &mut context)
        });
        assert!(result.unwrap_err().to_string().contains(resource));
        assert_eq!(large, 0);
    }
}

#[test]
fn temporary_concat_overlap_rejects_before_growing_combined_storage() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        temporaries: TemporaryLimits {
            payload_bytes: length,
            ..TemporaryLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .set_input_variables(BTreeMap::from([
            ("left".into(), Literal::String("x".repeat(length / 2))),
            ("right".into(), Literal::String("y".repeat(length / 2))),
        ]))
        .unwrap();
    let program = Program::parse("concat", "|x| = |left+right|").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("temporary value payload bytes"));
    assert_eq!(large, 0);
}

#[test]
fn builtin_log_temporary_rejection_avoids_its_output_copy() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        temporaries: TemporaryLimits {
            payload_bytes: length,
            ..TemporaryLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context.init_statements();
    context
        .set_input_variables(BTreeMap::from([(
            "value".into(),
            Literal::String("x".repeat(length)),
        )]))
        .unwrap();
    let program = Program::parse("log", "Log |value|").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("temporary value payload bytes"));
    assert_eq!(large, 1, "Only the evaluated argument is copied");
}

#[test]
fn builtin_variable_lookup_admits_its_copy_before_cloning_the_payload() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    for (allowance, copies) in [(length - 1, 0), (length + 5, 1)] {
        let mut context = Context::with_limits(RunLimits {
            temporaries: TemporaryLimits {
                payload_bytes: allowance,
                ..TemporaryLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        context.init_statements();
        context
            .set_input_variables(BTreeMap::from([(
                "value".into(),
                Literal::String("x".repeat(length)),
            )]))
            .unwrap();
        let program = Program::parse("get", "Get Variable |\"value\"|").unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        assert_eq!(result.is_ok(), copies == 1);
        assert_eq!(large, copies, "no payload clone before temporary admission");
    }
}

#[test]
fn format_measurement_does_not_sort_maps_before_output_admission() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    for allowed in [false, true] {
        let mut context = Context::with_limits(RunLimits {
            values: ValueLimits {
                string_bytes: if allowed { 1024 * 1024 } else { 16 },
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        context.init_statements();
        context
            .set_input_variables(BTreeMap::from([(
                "value".into(),
                Literal::Map(
                    (0..4096)
                        .map(|index| (format!("key{index:05}"), Literal::None))
                        .collect(),
                ),
            )]))
            .unwrap();
        let program =
            Program::parse("map-format", "Format String |\"{0}\"| With |[value]|").unwrap();
        let (result, copies) = observe(64 * 1024, || {
            evaluate_program_detailed(&program, &mut context)
        });
        assert_eq!(result.is_ok(), allowed, "{result:?}");
        assert_eq!(
            copies,
            if allowed { 3 } else { 1 },
            "input map table, then admitted output buffer and sorting slots"
        );
    }
}

#[test]
fn missing_format_fields_admit_diagnostic_text_before_copying_the_field_name() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticLimits,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 128,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    context.init_statements();
    context
        .set_input_variables(BTreeMap::from([(
            "template".into(),
            Literal::String(format!("{{{}}}", "x".repeat(length))),
        )]))
        .unwrap();
    let program = Program::parse("field", "Format String |template| With |{}|").unwrap();
    let (result, copies) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("diagnostic text bytes"));
    assert_eq!(copies, 1, "only the evaluated template argument is copied");
}

#[test]
#[cfg(target_os = "linux")]
fn process_preflight_rejects_before_copying_large_environment_or_capture_values() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::default();
    for (length, extra, expected) in [
        (1024 * 1024, "", "process command bytes"),
        (
            128 * 1024,
            " Options |{\"stdout_limit\": 1048577}|",
            "value string bytes",
        ),
    ] {
        let mut counts = Vec::new();
        for launch in [false, true] {
            let options = RunOptions {
                working_directory: Some(dir.path().into()),
                inherit_environment: false,
                environment: BTreeMap::from([("BIG".into(), Some("x".repeat(length).into()))]),
                ..Default::default()
            };
            let source = if launch {
                format!("Run Process |\"/bin/touch\"| With Arguments |[\"marker\"]|{extra}")
            } else {
                "No Operation".into()
            };
            let (result, copies) = observe(length, || {
                engine.run_source("process-admission", &source, options)
            });
            if launch {
                assert_eq!(result.outcome(), RunOutcome::LimitExceeded);
                assert!(result.result.unwrap_err().to_string().contains(expected));
            } else {
                assert!(result.result.is_ok());
            }
            counts.push(copies);
        }
        assert_eq!(
            counts[0], counts[1],
            "only the run's original environment snapshot is copied"
        );
        assert!(!dir.path().join("marker").exists());
    }
}

#[test]
fn filesystem_reads_reject_metadata_size_before_large_content_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let length = 128 * 1024;
    std::fs::write(directory.path().join("big"), vec![b'x'; length]).unwrap();
    let engine = Engine::default();
    for allowed in [false, true] {
        let options = RunOptions {
            working_directory: Some(directory.path().into()),
            inherit_environment: false,
            limits: RunLimits {
                values: ValueLimits {
                    string_bytes: length - usize::from(!allowed),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let (result, copies) = observe(length, || {
            engine.run_source("read", "Read File |\"big\"|", options)
        });
        assert_eq!(result.result.is_ok(), allowed, "{:?}", result.result);
        assert_eq!(copies, usize::from(allowed));
    }
}

#[test]
fn environment_results_admit_payload_before_copying_snapshot_values() {
    let directory = tempfile::tempdir().unwrap();
    let length = 128 * 1024;
    let engine = Engine::default();
    for source in [
        "Get Environment Variable |\"BIG\"|",
        "Environment Variables",
    ] {
        let mut counts = Vec::new();
        for allowed in [false, true] {
            let options = RunOptions {
                working_directory: Some(directory.path().into()),
                inherit_environment: false,
                environment: BTreeMap::from([("BIG".into(), Some("x".repeat(length).into()))]),
                limits: RunLimits {
                    values: ValueLimits {
                        string_bytes: length - usize::from(!allowed),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            let (result, copies) =
                observe(length, || engine.run_source("environment", source, options));
            assert_eq!(result.result.is_ok(), allowed, "{:?}", result.result);
            counts.push(copies);
        }
        assert_eq!(
            counts[1],
            counts[0] + 1,
            "only the admitted result copies the immutable snapshot value"
        );
    }
}

#[test]
fn datetime_formatting_rejects_before_any_large_result_or_intermediate_allocation() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let output_bytes = 160_000;
    let program = Program::parse(
        "date-allocation",
        "Format Date Time |\"2024-01-01T00:00:00Z\"| Using |pattern| In |\"UTC\"|",
    )
    .unwrap();
    for temporary in [false, true] {
        for allowed in [false, true] {
            let deficit = usize::from(!allowed);
            let mut context = Context::with_limits(RunLimits {
                values: ValueLimits {
                    string_bytes: output_bytes - if temporary { 0 } else { deficit },
                    ..Default::default()
                },
                temporaries: TemporaryLimits {
                    payload_bytes: output_bytes + 80_000 + 23 - if temporary { deficit } else { 0 },
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            context.init_statements();
            context
                .set_input_variables(BTreeMap::from([(
                    "pattern".into(),
                    Literal::String("%Y".repeat(40_000)),
                )]))
                .unwrap();
            let (result, copies) = observe(128 * 1024, || {
                evaluate_program_detailed(&program, &mut context)
            });
            assert_eq!(result.is_ok(), allowed, "temporary={temporary}: {result:?}");
            assert_eq!(
                copies,
                usize::from(allowed),
                "only admitted final output may allocate this many bytes"
            );
        }
    }
}

#[test]
fn string_outputs_are_admitted_before_format_join_replace_and_regex_payload_copies() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    for (source, allowance, accepted_copies) in [
        (
            "Format String |\"{0}{0}\"| With |[value]|",
            3 * length + 6,
            2,
        ),
        (
            "Join Strings |[value, \"z\"]| With |\",\"|",
            2 * length + 4,
            2,
        ),
        (
            "Replace String |value| Find |\"x\"| With |\"yy\"|",
            3 * length + 3,
            2,
        ),
        ("Split String |value| On |\",\"|", 2 * length + 1, 2),
        ("Find Matches In |value| Regex |\"x+\"|", 2 * length + 2, 2),
        ("Capture From |value| Regex |\"(x+)\"|", 3 * length + 4, 3),
    ] {
        for allowed in [false, true] {
            let mut context = Context::with_limits(RunLimits {
                temporaries: TemporaryLimits {
                    payload_bytes: allowance - usize::from(!allowed),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            context.init_statements();
            context
                .set_input_variables(BTreeMap::from([(
                    "value".into(),
                    Literal::String("x".repeat(length)),
                )]))
                .unwrap();
            let program = Program::parse("string-allocation", source).unwrap();
            let (result, copies) =
                observe(length, || evaluate_program_detailed(&program, &mut context));
            assert_eq!(result.is_ok(), allowed, "{source}: {result:?}");
            assert_eq!(
                copies,
                if allowed { accepted_copies } else { 1 },
                "{source}: rejected output copies no payload beyond its input"
            );
        }
    }
}

#[test]
fn expanding_unicode_case_conversion_checks_output_before_allocating() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        temporaries: TemporaryLimits {
            payload_bytes: length * 5 / 2 - 1,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    context.init_statements();
    context
        .set_input_variables(BTreeMap::from([(
            "value".into(),
            Literal::String("İ".repeat(length / 2)),
        )]))
        .unwrap();
    let program = Program::parse("case-allocation", "Lowercase String |value|").unwrap();
    let (result, copies) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("temporary value payload bytes"));
    assert_eq!(copies, 1, "only the evaluated argument is copied");
}

#[test]
fn collection_output_admission_precedes_payload_copies_and_count_sized_allocation() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let length = 64 * 1024;
    for (source, allowance, copies, accepted) in [
        ("Repeat |value| Times |2|", 3 * length + 3, 1, false),
        ("Repeat |value| Times |2|", 3 * length + 4, 3, true),
        ("Repeat |value| Times |2147483647|", usize::MAX, 1, false),
        (
            "Append To |[value]| Value |value|",
            4 * length - 1,
            2,
            false,
        ),
        ("Append To |[value]| Value |value|", 4 * length, 4, true),
        ("Get From |[value]| At |0|", 2 * length + 3, 1, false),
        ("Get From |[value]| At |0|", 2 * length + 4, 2, true),
    ] {
        let mut context = Context::with_limits(RunLimits {
            temporaries: TemporaryLimits {
                payload_bytes: allowance,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        context.init_statements();
        context
            .set_input_variables(BTreeMap::from([(
                "value".into(),
                Literal::String("x".repeat(length)),
            )]))
            .unwrap();
        let program = Program::parse("collection-allocation", source).unwrap();
        let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
        assert_eq!(result.is_ok(), accepted, "{source}: {result:?}");
        assert_eq!(
            large, copies,
            "{source}: only admitted input/result payloads are copied"
        );
    }
}

#[test]
fn collection_access_diagnostics_reject_large_key_details_before_copying_them() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticLimits,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 128,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap();
    context.init_statements();
    context
        .set_input_variables(BTreeMap::from([(
            "key".into(),
            Literal::String("x".repeat(length)),
        )]))
        .unwrap();
    let program = Program::parse("lookup", "Get From |{}| At |key|").unwrap();
    let (result, copies) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("diagnostic text bytes"));
    assert_eq!(copies, 1, "only the evaluated key argument is copied");
}

#[test]
fn map_key_iteration_checks_string_limits_before_copying_keys_into_the_result() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    for allowed in [false, true] {
        for statement in ["Map Keys |value|", "Map Entries |value|"] {
            let mut context = Context::with_limits(RunLimits {
                values: ValueLimits {
                    string_bytes: length - usize::from(!allowed),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            context.init_statements();
            context
                .set_input_variables(BTreeMap::from([(
                    "value".into(),
                    Literal::Map(std::collections::HashMap::from([(
                        "x".repeat(length),
                        Literal::None,
                    )])),
                )]))
                .unwrap();
            let program = Program::parse("key-allocation", statement).unwrap();
            let (result, large) =
                observe(length, || evaluate_program_detailed(&program, &mut context));
            assert_eq!(result.is_ok(), allowed, "{statement}: {result:?}");
            assert_eq!(
                large,
                1 + usize::from(allowed),
                "input key plus admitted output key"
            );
        }
    }
}

#[test]
fn container_construction_transfers_child_payload_without_copying_it() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    let string = "x".repeat(length);
    let pointer = string.as_ptr();
    let owned = Mutex::new(Some(Literal::String(string)));
    let mut context = Context::default();
    context
        .register_native("Host", move |_| Ok(owned.lock().unwrap().take().unwrap()))
        .unwrap();
    evaluate_program_detailed(
        &Program::parse("definition", "Wrap { Return |[@{ Host }]| }").unwrap(),
        &mut context,
    )
    .unwrap();
    let program = Program::parse("call", "Wrap").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    let Literal::Array(values) = result.unwrap() else {
        panic!()
    };
    let Literal::String(value) = &values[0] else {
        panic!()
    };
    assert_eq!(value.as_ptr(), pointer);
    assert_eq!(large, 0);
}

#[test]
fn temporary_argument_slots_reject_before_allocating_a_wide_argument_vector() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::TemporaryLimits,
    };
    let mut context = Context::with_limits(RunLimits {
        temporaries: TemporaryLimits {
            values: 0,
            ..TemporaryLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let parameters = (0..4096).map(|i| format!(" |p{i}|")).collect::<String>();
    context
        .register_native(&format!("Take{parameters}"), |_| {
            panic!("unreachable callback")
        })
        .unwrap();
    let program =
        Program::parse("wide-arguments", &format!("Take{}", " |0|".repeat(4096))).unwrap();
    let (result, large) = observe(16 * 1024, || {
        evaluate_program_detailed(&program, &mut context)
    });
    assert!(result.unwrap_err().to_string().contains("temporary values"));
    assert_eq!(large, 0);
}

#[test]
fn unique_engine_input_payload_moves_into_results_without_copying() {
    let length = 64 * 1024;
    let value = "x".repeat(length);
    let pointer = value.as_ptr();
    let options = RunOptions {
        inherit_environment: false,
        variables: BTreeMap::from([("x".into(), Literal::String(value))]),
        ..RunOptions::default()
    };
    let engine = Engine::default();
    let (run, large) = observe(length, || engine.run_source("move", "", options));
    assert_eq!(run.outcome(), RunOutcome::Succeeded);
    assert!(run.snapshot_error.is_none());
    let Literal::String(value) = &run.variables["x"] else {
        panic!()
    };
    assert_eq!(value.as_ptr(), pointer);
    assert_eq!(large, 0);
}

#[test]
fn result_rejection_avoids_root_payload_copies() {
    use botwork::core::run::ResultLimits;
    let length = 64 * 1024;
    let options = RunOptions {
        inherit_environment: false,
        variables: BTreeMap::from([("x".into(), Literal::String("x".repeat(length)))]),
        limits: RunLimits {
            results: ResultLimits {
                payload_bytes: 0,
                ..ResultLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let engine = Engine::default();
    let (run, large) = observe(length, || engine.run_source("reject", "", options));
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run.variables.is_empty());
    assert!(run
        .snapshot_error
        .unwrap()
        .to_string()
        .contains("result payload bytes"));
    assert_eq!(large, 0);
}

#[test]
fn assignment_export_has_only_native_and_stored_payload_allocations() {
    use botwork::core::run::ResultLimits;
    let length = 64 * 1024;
    let mut engine = Engine::default();
    engine
        .register_native("Host", move |_, _| Ok(Literal::String("x".repeat(length))))
        .unwrap();
    for payload_bytes in [length, 2 * length] {
        let options = RunOptions {
            inherit_environment: false,
            limits: RunLimits {
                results: ResultLimits {
                    payload_bytes,
                    ..ResultLimits::default()
                },
                ..RunLimits::default()
            },
            ..RunOptions::default()
        };
        let (run, large) = observe(length, || {
            engine.run_source("assignment", "|x| = Host", options)
        });
        assert_eq!(run.snapshot_error.is_some(), payload_bytes == length);
        assert_eq!(
            large, 2,
            "Native return and stored assignment; no final root copy"
        );
    }
}

#[test]
fn checked_context_copy_rejects_before_allocating_variable_table() {
    use botwork::core::{eval::Context, run::SnapshotLimits};
    let mut context = Context::with_limits(RunLimits {
        snapshots: SnapshotLimits {
            entries: 0,
            ..SnapshotLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    context
        .set_input_variables(
            (0..4096)
                .map(|i| (format!("v{i}"), Literal::None))
                .collect(),
        )
        .unwrap();
    let (result, large) = observe(16 * 1024, || context.try_clone());
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("snapshot table entries"));
    assert_eq!(large, 0);
}

#[test]
fn engine_snapshot_rejects_before_copying_template_table() {
    use botwork::core::run::SnapshotLimits;
    let mut engine = Engine::default();
    for i in 0..1024 {
        engine
            .register_native(&format!("Host {i}"), |_, _| Ok(Literal::None))
            .unwrap();
    }
    let options = RunOptions {
        inherit_environment: false,
        limits: RunLimits {
            snapshots: SnapshotLimits {
                entries: 0,
                ..SnapshotLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let (run, large) = observe(16 * 1024, || engine.run_source("snapshot", "", options));
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert!(run
        .result
        .unwrap_err()
        .to_string()
        .contains("snapshot table entries"));
    assert_eq!(large, 0);
}

#[test]
fn imported_snapshot_rejects_before_copying_wide_module_frame() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::SnapshotLimits,
    };
    let directory =
        std::env::temp_dir().join(format!("botwork-frame-allocation-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("wide.botwork");
    let source = (0..4096)
        .map(|i| format!("|v{i}| = |0|\n"))
        .collect::<String>()
        + "Read {}";
    std::fs::write(&path, source).unwrap();
    let mut context = Context::with_limits(RunLimits {
        snapshots: SnapshotLimits {
            entries: 1,
            ..SnapshotLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let program = Program::parse(
        "import",
        &format!(
            "Import |{}| As |m|",
            serde_json::to_string(path.to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();
    evaluate_program_detailed(&program, &mut context).unwrap();
    let call = Program::parse("call", "m::Read").unwrap();
    let (result, large) = observe(16 * 1024, || evaluate_program_detailed(&call, &mut context));
    std::fs::remove_dir_all(directory).unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("snapshot table entries"));
    assert_eq!(large, 0);
}

#[test]
fn retained_value_failure_precedes_the_assignment_copy() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::RetainedValueLimits,
    };
    let length = 64 * 1024;
    let mut context = Context::with_limits(RunLimits {
        retained_values: RetainedValueLimits {
            values: 0,
            ..RetainedValueLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let payload = Literal::String("x".repeat(length));
    context
        .register_native("Host", move |_| Ok(payload.clone()))
        .unwrap();
    let program = Program::parse("allocation", "|x| = Host").unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result.unwrap_err().to_string().contains("retained values"));
    assert_eq!(large, 1, "Only the host result allocates its string");
}

#[test]
fn definition_retention_failure_precedes_signature_parameter_copies() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::RetainedDefinitionLimits,
    };
    let length = 64 * 1024;
    let source = format!("Read |{}| {{}}", "x".repeat(length));
    let program = Program::parse("parameter", &source).unwrap();
    let mut context = Context::with_limits(RunLimits {
        retained_definitions: RetainedDefinitionLimits {
            definitions: 0,
            ..RetainedDefinitionLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("retained definitions"));
    assert_eq!(large, 0);
    assert!(context.statement_signature("Read |x|").unwrap().is_none());
}

#[test]
fn variable_name_failure_precedes_key_and_assignment_copying() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::RetainedNameLimits,
    };
    let length = 64 * 1024;
    let program = Program::parse(
        "name",
        &format!("|{}| = |\"{}\"|", "x".repeat(length), "y".repeat(length)),
    )
    .unwrap();
    let mut context = Context::with_limits(RunLimits {
        retained_names: RetainedNameLimits {
            name_bytes: 4,
            ..RetainedNameLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("variable name bytes"));
    assert_eq!(
        large, 1,
        "Only evaluating the string RHS copies its payload"
    );
}

#[test]
fn context_snapshots_share_variable_name_text_without_payload_copies() {
    use botwork::core::eval::Context;
    let length = 64 * 1024;
    let mut context = Context::default();
    context
        .set_input_variables(BTreeMap::from([("x".repeat(length), Literal::None)]))
        .unwrap();
    let (clone, large) = observe(length, || context.clone());
    assert_eq!(large, 0);
    assert!(clone.checkpoint().is_ok());
}

#[test]
fn registry_failure_precedes_dsl_parameter_metadata_copying() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
        run::RetainedRegistryLimits,
    };
    let length = 64 * 1024;
    let program =
        Program::parse("registry", &format!("Read |{}| {{}}", "x".repeat(length))).unwrap();
    let mut context = Context::with_limits(RunLimits {
        retained_registry: RetainedRegistryLimits {
            entries: 0,
            ..RetainedRegistryLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    assert!(result.unwrap_err().to_string().contains("registry entries"));
    assert_eq!(large, 0);
}

#[test]
fn context_clones_share_registry_keys_and_metadata_payloads() {
    use botwork::core::{eval::Context, signature::StatementSignature};
    let length = 64 * 1024;
    let header = "x".repeat(length);
    let signature = StatementSignature::native(&header)
        .unwrap()
        .description("y".repeat(length));
    let mut context = Context::default();
    context
        .register_native_with_signature(signature, |_| Ok(Literal::None))
        .unwrap();
    let (clone, large) = observe(length, || context.clone());
    assert_eq!(large, 0);
    assert_eq!(clone.statement_signatures().len(), 1);
}

#[test]
fn completion_normalization_does_not_copy_oversized_host_prefixes() {
    use botwork::core::eval::Context;
    let length = 64 * 1024;
    let mut context = Context::default();
    context
        .register_native("Read", |_| Ok(Literal::None))
        .unwrap();
    for prefix in ["x".repeat(length), " \t".repeat(length), "İ".repeat(length)] {
        let (matches, large) = observe(length, || context.complete_statements(&prefix));
        assert_eq!(large, 0);
        assert_eq!(matches.len(), usize::from(prefix.starts_with(' ')));
    }
}

#[test]
fn fresh_engine_admission_shares_template_documentation_storage() {
    use botwork::core::signature::StatementSignature;
    let length = 64 * 1024;
    let mut engine = Engine::default();
    engine
        .register_native_with_signature(
            StatementSignature::native("Host")
                .unwrap()
                .description("x".repeat(length)),
            |_, _| Ok(Literal::None),
        )
        .unwrap();
    let (result, large) = observe(length, || {
        engine.run_source("host", "Host", RunOptions::default())
    });
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(large, 0);
}

#[test]
fn imported_native_bindings_share_the_original_registry_key() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    let mut context = Context::default();
    context
        .register_native(&"x".repeat(length), |_| Ok(Literal::None))
        .unwrap();
    let directory = std::env::temp_dir().join(format!(
        "botwork-registry-allocation-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let module = directory.join("empty.botwork");
    std::fs::write(&module, "").unwrap();
    let source = format!(
        "Import |{}| As |empty|",
        serde_json::to_string(module.to_str().unwrap()).unwrap()
    );
    let program = Program::parse("import", &source).unwrap();
    let (result, large) = observe(length, || evaluate_program_detailed(&program, &mut context));
    std::fs::remove_file(module).unwrap();
    std::fs::remove_dir(directory).unwrap();
    result.unwrap();
    assert_eq!(large, 0);
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

#[test]
fn json_preflight_avoids_escape_buffers_and_wide_raw_arrays() {
    use botwork::core::{
        diagnostic::DiagnosticCode,
        input::{parse_variable_with_limits, InputLimits},
    };
    let length = 64 * 1024;
    let setting = format!("x=\"{}\"", r"\u0061".repeat(length));
    let limits = InputLimits {
        values: ValueLimits {
            string_bytes: 3,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    let (result, large) = observe(length, || {
        parse_variable_with_limits("escaped", &setting, &limits)
    });
    assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
    assert_eq!(large, 0);
    let setting = format!("x=[{}null]", "null,".repeat(10_000));
    let limits = InputLimits {
        values: ValueLimits {
            entries: 8,
            ..ValueLimits::default()
        },
        ..InputLimits::default()
    };
    let (result, large) = observe(1024, || {
        parse_variable_with_limits("array", &setting, &limits)
    });
    assert_eq!(result.unwrap_err().code(), DiagnosticCode::ResourceLimit);
    assert_eq!(large, 0);
}

#[test]
fn aggregate_outgoing_admission_precedes_native_call_stack_signature_copy() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticCode,
        eval::{evaluate_program_detailed, Context},
        grammar::BWErr,
        run::RetainedDiagnosticLimits,
    };
    let signature = "A".repeat(64 * 1024);
    let mut allocations = Vec::new();
    for records in [1, 2] {
        let mut context = Context::with_limits(RunLimits {
            retained_diagnostics: RetainedDiagnosticLimits {
                records,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        context
            .register_native(&signature, |_| Err(BWErr::NativeError("failed".into())))
            .unwrap();
        let program = Program::parse("outgoing", &signature).unwrap();
        let (result, copies) = observe(signature.len(), || {
            evaluate_program_detailed(&program, &mut context)
        });
        let error = result.unwrap_err();
        assert_eq!(
            error.code(),
            if records == 1 {
                DiagnosticCode::ResourceLimit
            } else {
                DiagnosticCode::Native
            }
        );
        if records == 1 {
            assert_eq!(error.causes[0].omissions.as_ref().unwrap().call_frames, 1);
        } else {
            assert_eq!(error.call_stack.len(), 1);
        }
        allocations.push(copies);
    }
    // Both enter the call; only the admitted error copies its complete snapshot.
    assert_eq!(allocations[1], allocations[0] + 1);
}

#[test]
fn aggregate_construction_rejects_initial_borrowed_and_grouped_messages_before_large_copies() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticCode,
        eval::{evaluate_program_detailed, Context},
        run::RetainedDiagnosticLimits,
    };
    let length = 64 * 1024;
    let name = "x".repeat(length);
    for grouped in [false, true] {
        let source = if grouped {
            format!("|data| = |{{}}|\n|out| = |data.{name}|")
        } else {
            name.clone()
        };
        let program = Program::parse("construction", &source).unwrap();
        let mut counts = Vec::new();
        for admitted in [false, true] {
            let mut context = Context::with_limits(RunLimits {
                retained_diagnostics: RetainedDiagnosticLimits {
                    text_bytes: if admitted { 3 * length } else { length + 1 },
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            let (result, copies) =
                observe(length, || evaluate_program_detailed(&program, &mut context));
            let error = result.unwrap_err();
            let category = if grouped {
                DiagnosticCode::CollectionAccess
            } else {
                DiagnosticCode::UndefinedStatement
            };
            if admitted {
                assert_eq!(error.code(), category);
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), category);
            }
            counts.push(copies);
        }
        assert_eq!(counts[0], 0);
        assert_eq!(counts[1], if grouped { 2 } else { 1 });
    }
}

#[test]
fn shared_ast_construction_quotas_reject_before_large_message_and_prefix_copies() {
    use botwork::core::{diagnostic::DiagnosticCode, run::RetainedDiagnosticLimits};
    let filename = "é".repeat(128 * 1024);
    for (source, kind) in [
        (format!("{}Log |1 + 2|", " ".repeat(128 * 1024)), 0),
        (format!("{}|x| = |1 +|", " \t".repeat(128 * 1024)), 1),
        ("Return".into(), 2),
        ("Read |x| and |x| {}".into(), 3),
    ] {
        let mut accepted_copies = None;
        for deficit in 0..=2 {
            let (run, copies) = observe(64 * 1024, || {
                Engine::default().run_source(
                    &filename,
                    &source,
                    RunOptions {
                        limits: RunLimits {
                            syntax: botwork::core::syntax_limits::SyntaxLimits {
                                operators: if kind == 0 {
                                    0
                                } else {
                                    RunLimits::default().syntax.operators
                                },
                                ..Default::default()
                            },
                            retained_diagnostics: match deficit {
                                1 => RetainedDiagnosticLimits {
                                    text_bytes: 0,
                                    ..Default::default()
                                },
                                2 => RetainedDiagnosticLimits {
                                    source_bytes: 0,
                                    ..Default::default()
                                },
                                _ => RetainedDiagnosticLimits::default(),
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
            });
            let error = run.result.unwrap_err();
            if deficit == 0 {
                accepted_copies = Some(copies);
            } else {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert!(error.causes[0].omissions.is_some());
                // Guard rejection owns no source; parser-owned buffers remain necessary
                // for the other paths, but their complete error strings are skipped.
                assert_eq!(
                    accepted_copies.unwrap() - copies,
                    match kind {
                        0 | 3 => 2,
                        _ => 1,
                    }
                );
                if kind == 0 {
                    assert_eq!(copies, 0);
                }
            }
        }
    }
}

#[test]
fn aggregate_operation_panic_limits_reject_before_large_signature_detail_allocation() {
    use botwork::core::{
        diagnostic::DiagnosticCode,
        operation::{NativeOperation, OperationBudget, OperationControl, OperationOwnershipLimits},
        signature::StatementSignature,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let header = "x".repeat(128 * 1024);
    for factory in [false, true] {
        for reject in [false, true] {
            let operation = NativeOperation::asynchronous(
                StatementSignature::native(&header).unwrap(),
                move |_, _| {
                    assert!(!factory, "factory panic");
                    async { panic!("poll panic") }
                },
            )
            .unwrap()
            .with_ownership_budget(OperationBudget::new(OperationOwnershipLimits {
                text_bytes: if reject { 0 } else { usize::MAX },
                ..Default::default()
            }));
            let (result, copies) = observe(64 * 1024, || {
                runtime.block_on(operation.invoke(vec![], OperationControl::default()))
            });
            let error = result.unwrap_err();
            if reject {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::NativePanic);
                assert_eq!(copies, 0);
            } else {
                assert_eq!(error.code(), DiagnosticCode::NativePanic);
                assert_eq!(copies, 1);
            }
            assert_eq!(operation.ownership_budget().usage(), Default::default());
        }
    }
}

#[test]
fn assertion_operand_artifacts_are_admitted_before_full_payload_copies() {
    use botwork::core::{
        ast::Program,
        diagnostic::DiagnosticCode,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 64 * 1024;
    for admitted in [false, true] {
        let mut limits = RunLimits::default();
        if !admitted {
            limits.diagnostics.text_bytes = 1024;
        }
        let mut context = Context::with_limits(limits).unwrap();
        context.init_statements();
        context
            .set_input_variables(BTreeMap::from([
                ("actual".into(), Literal::String("a".repeat(length))),
                ("expected".into(), Literal::String("b".repeat(length))),
            ]))
            .unwrap();
        let program = Program::parse("artifacts", "Assert |actual| Equals |expected|").unwrap();
        let (result, allocations) =
            observe(length, || evaluate_program_detailed(&program, &mut context));
        let error = result.unwrap_err();
        assert_eq!(
            error.code(),
            if admitted {
                DiagnosticCode::Assertion
            } else {
                DiagnosticCode::ResourceLimit
            }
        );
        assert_eq!(
            allocations,
            if admitted { 4 } else { 2 },
            "two argument copies, then two admitted artifact strings"
        );
    }
}

#[test]
fn parse_json_admits_decoded_strings_before_allocating_them() {
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let length = 100_000;
    let json = format!("\"{}\"", "x".repeat(length));
    let mut counts = Vec::new();
    for admitted in [false, true] {
        let mut limits = RunLimits::default();
        // The argument copy (the JSON text) fits; its decoded String does not.
        limits.temporaries.payload_bytes = if admitted {
            4 * length
        } else {
            json.len() + length / 2
        };
        let mut context = Context::with_limits(limits).unwrap();
        context.init_statements();
        context
            .set_input_variables(BTreeMap::from([(
                "json".into(),
                Literal::String(json.clone()),
            )]))
            .unwrap();
        let program = Program::parse("json", "Parse JSON |json|").unwrap();
        let (result, allocations) =
            observe(length, || evaluate_program_detailed(&program, &mut context));
        if !admitted {
            assert_eq!(
                result.as_ref().unwrap_err().code(),
                botwork::core::diagnostic::DiagnosticCode::ResourceLimit
            );
        }
        counts.push((result.is_ok(), allocations));
    }
    assert_eq!(
        counts,
        [(false, 1), (true, 2)],
        "the argument copy, then the decoded String only after admission"
    );
}
