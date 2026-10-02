#[path = "support/cli_harness.rs"]
mod cli_harness;

use botwork::core::{
    ast::Program,
    diagnostic::{DiagnosticCode, DiagnosticResult},
    eval::{evaluate_program_detailed, Context},
    grammar::Literal,
};
use cli_harness::Harness;
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

struct Project(Harness);

#[test]
fn imported_syntax_guards_admit_calls_before_source_prefix_copy_and_keep_original_limits() {
    use botwork::core::{
        diagnostic::DiagnosticLimits, grammar::BWErr, run::RunLimits, syntax_limits::SyntaxLimits,
    };
    let project = Project::new();
    project.write("nested.botwork", "Effect\nLog |[[1]]|");
    project.write("good.botwork", "Value { Return |7| }");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut context = Context::with_limits(RunLimits {
        syntax: SyntaxLimits {
            nesting: 2,
            operators: 64,
        },
        diagnostics: DiagnosticLimits {
            call_frames: 0,
            ..DiagnosticLimits::default()
        },
        ..RunLimits::default()
    })
    .unwrap();
    let seen = calls.clone();
    context
        .register_native("Effect", move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let mut sibling = context.clone();
    let error = project
        .run(
            "Effect\nRead { Import |\"nested.botwork\"| As |lib| }\nTry { Read } Catch { Effect }",
            &mut context,
        )
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert!(matches!(
        error.causes[0].error.as_ref(),
        BWErr::ResourceLimit {
            resource: "syntax nesting",
            limit: 2
        }
    ));
    assert!(error.causes[0].span.is_none());
    let omitted = error.causes[0].omissions.as_ref().unwrap();
    assert_eq!(omitted.call_frames, 1);
    assert_eq!(omitted.related_locations, 1);
    assert_eq!(omitted.detail_fields, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(context.checkpoint().is_err());
    assert_eq!(
        project
            .run(
                "Import |\"good.botwork\"| As |lib|\nlib::Value",
                &mut sibling
            )
            .unwrap()
            .to_string(),
        "7"
    );
}

#[test]
fn entry_file_syntax_errors_use_installed_diagnostic_quotas_and_keep_input_snapshots() {
    use botwork::core::{
        diagnostic::DiagnosticLimits,
        run::{Engine, RunLimits, RunOptions, RunOutcome},
    };
    let project = Project::new();
    let path = project.write("syntax.botwork", "|x| = |1 +|");
    let settings = || RunOptions {
        variables: std::collections::BTreeMap::from([("seed".into(), Literal::Int(7))]),
        limits: RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let engine = Engine::default();
    let run = engine.run_file(&path, settings());
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(run.steps, 0);
    assert_eq!(run.variables["seed"].to_string(), "7");
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::Syntax);
    assert_eq!(
        error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .file,
        path.to_str().unwrap()
    );
    let valid = project.write("valid.botwork", "|answer| = |7|");
    assert_eq!(
        engine.run_file(valid, settings()).outcome(),
        RunOutcome::Succeeded
    );
}

#[test]
fn imported_syntax_construction_admits_calls_then_unwinds_sites_without_running_module_effects() {
    use botwork::core::{diagnostic::DiagnosticLimits, run::RunLimits};
    let project = Project::new();
    project.write("parent.botwork", "Import |\"invalid.botwork\"| As |bad|");
    project.write("invalid.botwork", "Effect\n|x| = |1 +|");
    project.write("good.botwork", "Value { Return |7| }");
    for reject in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut context = Context::with_limits(RunLimits {
            diagnostics: if reject {
                DiagnosticLimits {
                    call_frames: 0,
                    ..DiagnosticLimits::default()
                }
            } else {
                DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let seen = calls.clone();
        context
            .register_native("Effect", move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(Literal::None)
            })
            .unwrap();
        let mut sibling = context.clone();
        let source = if reject {
            "Effect\nRead { Import |\"parent.botwork\"| As |lib| }\nTry { Read } Catch { Effect }\nEffect"
        } else {
            "Effect\nRead { Import |\"parent.botwork\"| As |lib| }\nRead\nEffect"
        };
        let error = project.run(source, &mut context).unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        if reject {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::Syntax);
            let omissions = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omissions.call_frames, 1);
            assert_eq!(omissions.related_locations, 2);
            assert_eq!(omissions.detail_fields, 1);
            assert!(context.checkpoint().is_err());
        } else {
            assert_eq!(error.code(), DiagnosticCode::Syntax);
            assert_eq!(error.call_stack.len(), 1);
            assert_eq!(error.related.len(), 2);
            assert!(context.checkpoint().is_ok());
        }
        assert_eq!(
            project
                .run(
                    "Import |\"good.botwork\"| As |lib|\nlib::Value",
                    &mut sibling
                )
                .unwrap()
                .to_string(),
            "7"
        );
        // A successful later import can use the same alias after failed publication.
        if !reject {
            assert_eq!(
                project
                    .run(
                        "Import |\"good.botwork\"| As |lib|\nlib::Value",
                        &mut context
                    )
                    .unwrap()
                    .to_string(),
                "7"
            );
        }
    }
}

#[test]
fn module_validation_admits_entered_calls_before_details_and_preserves_pre_import_effects() {
    use botwork::core::{diagnostic::DiagnosticLimits, run::RunLimits};
    let project = Project::new();
    for (module, code, fields) in [
        ("Effect\nBreak", DiagnosticCode::InvalidControl, 1),
        (
            "Effect\nRead |é| with |é| {}",
            DiagnosticCode::DuplicateParameter,
            2,
        ),
    ] {
        project.write("invalid.botwork", module);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                call_frames: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let seen = calls.clone();
        context
            .register_native("Effect", move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(Literal::None)
            })
            .unwrap();
        let mut sibling = context.clone();
        let error = project.run("Effect\nRead { Import |\"invalid.botwork\"| As |lib| }\nTry { Read } Catch { Effect }\nEffect", &mut context).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
        let original = &error.causes[0];
        assert_eq!(original.code(), code);
        let omissions = original.omissions.as_ref().unwrap();
        assert_eq!(omissions.call_frames, 1);
        assert_eq!(omissions.detail_fields, fields);
        assert_eq!(omissions.related_locations, if fields == 1 { 1 } else { 2 });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(context.checkpoint().is_err());
        project.write("invalid.botwork", "Value { Return |7| }");
        assert_eq!(
            project
                .run(
                    "Import |\"invalid.botwork\"| As |lib|\nlib::Value",
                    &mut sibling
                )
                .unwrap()
                .to_string(),
            "7"
        );
    }
}

#[test]
fn namespace_collision_admits_both_locations_before_loading_and_preserves_original_namespace() {
    use botwork::core::{diagnostic::DiagnosticLimits, grammar::BWErr, run::RunLimits};
    let project = Project::new();
    project.write("good.botwork", "Value { Return |7| }");
    for text_bytes in [0, DiagnosticLimits::default().text_bytes] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        project
            .run("Import |\"good.botwork\"| As |lib|", &mut context)
            .unwrap();
        let mut sibling = context.clone();
        // Collision must precede validation/loading of this invalid path.
        for source in ["Import |\"missing.txt\"| As |LIB|", "lib::Other {}"] {
            let mut copy = context.clone();
            let error = project.run(source, &mut copy).unwrap_err();
            if text_bytes == 0 {
                assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
                assert_eq!(error.causes[0].code(), DiagnosticCode::DuplicateNamespace);
                let omitted = error.causes[0].omissions.as_ref().unwrap();
                assert_eq!(omitted.detail_fields, 2);
                assert_eq!(omitted.related_locations, 1);
                let BWErr::DuplicateNamespace {
                    namespace,
                    original,
                    duplicate,
                } = error.causes[0].error.as_ref()
                else {
                    panic!("namespace")
                };
                assert_eq!(namespace, "lib");
                assert!(
                    original.contains("coordinates omitted")
                        && duplicate.contains("coordinates omitted")
                );
                assert!(copy.checkpoint().is_err());
            } else {
                assert_eq!(error.code(), DiagnosticCode::DuplicateNamespace);
                let BWErr::DuplicateNamespace {
                    namespace,
                    original,
                    duplicate,
                } = error.error.as_ref()
                else {
                    panic!("namespace")
                };
                assert_eq!(namespace, "lib");
                assert_eq!(original, &error.related[0].span.location());
                assert_eq!(duplicate, &error.span.as_ref().unwrap().location());
                assert_eq!(
                    project.run("lib::Value", &mut copy).unwrap().to_string(),
                    "7"
                );
            }
        }
        assert_eq!(
            project.run("lib::Value", &mut sibling).unwrap().to_string(),
            "7"
        );
    }
}

#[test]
fn import_read_construction_admits_message_call_and_originating_site_together() {
    use botwork::core::{diagnostic::DiagnosticLimits, run::RunLimits};
    let project = Project::new();
    fs::write(project.write("invalid.botwork", ""), [0xff]).unwrap();
    fs::create_dir(project.0.workspace.join("directory.botwork")).unwrap();
    for path in [
        "bad.txt",
        "https://example.invalid/a.botwork",
        "missing-é.botwork",
        "invalid.botwork",
        "directory.botwork",
    ] {
        let source = format!("Outer {{ Import |\"{path}\"| As |lib| }}\nOuter");
        let baseline = project.run(&source, &mut Context::default()).unwrap_err();
        assert_eq!(baseline.code(), DiagnosticCode::ImportRead);
        assert_eq!(baseline.call_stack.len(), 1);
        assert_eq!(baseline.related.len(), 1);
        assert_eq!(baseline.related[0].message, "imported here");
        assert_eq!(
            baseline.span.as_ref().unwrap().text(),
            format!("\"{path}\"")
        );
        let size = DiagnosticLimits::default().check(&baseline).unwrap();
        let exact = DiagnosticLimits {
            diagnostics: size.diagnostics,
            depth: size.depth,
            call_frames: size.call_frames,
            related_locations: size.related_locations,
            text_bytes: size.text_bytes,
            source_bytes: size.source_bytes,
        };
        let mut context = Context::with_limits(RunLimits {
            diagnostics: exact.clone(),
            ..RunLimits::default()
        })
        .unwrap();
        let error = project.run(&source, &mut context).unwrap_err();
        assert_eq!(
            error.to_value().to_string(),
            baseline.to_value().to_string()
        );
        assert!(context.checkpoint().is_ok());
        for diagnostics in [
            DiagnosticLimits {
                text_bytes: size.text_bytes - 1,
                ..exact.clone()
            },
            DiagnosticLimits {
                related_locations: 0,
                ..exact.clone()
            },
            DiagnosticLimits {
                source_bytes: size.source_bytes - 1,
                ..exact.clone()
            },
            DiagnosticLimits {
                call_frames: 0,
                ..exact.clone()
            },
        ] {
            let mut context = Context::with_limits(RunLimits {
                diagnostics,
                ..RunLimits::default()
            })
            .unwrap();
            let error = project.run(&source, &mut context).unwrap_err();
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
            let omitted = error.causes[0].omissions.as_ref().unwrap();
            assert_eq!(omitted.related_locations, 1);
            assert_eq!(omitted.call_frames, 1);
            assert!(context.checkpoint().is_err());
            assert!(context.complete_statements("lib::").is_empty());
        }
    }
}

#[test]
fn rejected_import_details_preserve_prior_effects_skip_catch_and_leave_fresh_runs_usable() {
    use botwork::core::{
        diagnostic::DiagnosticLimits,
        run::{Engine, RunLimits, RunOptions, RunOutcome},
    };
    let project = Project::new();
    project.write("good.botwork", "Value { Return |7| }");
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut engine = Engine::default();
    engine
        .register_native("Effect", move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    let options = || RunOptions {
        working_directory: Some(project.0.workspace.clone()),
        limits: RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes: 0,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        },
        ..RunOptions::default()
    };
    let source = "|error| = |7|\n|before| = |1|\nEffect\nTry { Import |\"bad.txt\"| As |lib| } Catch |error| { |caught| = |1|\nEffect }\n|after| = |2|";
    let run = engine.run_source("entry", source, options());
    assert_eq!(run.outcome(), RunOutcome::LimitExceeded);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(run.variables["error"].to_string(), "7");
    assert_eq!(run.variables["before"].to_string(), "1");
    assert!(!run.variables.contains_key("caught") && !run.variables.contains_key("after"));
    let error = run.result.unwrap_err();
    assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
    assert_eq!(
        error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .related_locations,
        1
    );
    let good = engine.run_source(
        "fresh",
        "Import |\"good.botwork\"| As |lib|\nlib::Value",
        options(),
    );
    assert_eq!(good.outcome(), RunOutcome::Succeeded);
    assert_eq!(good.result.unwrap().to_string(), "7");
}

#[test]
fn cycle_construction_preserves_chain_and_all_parent_sites_through_unwinding() {
    use botwork::core::{diagnostic::DiagnosticLimits, grammar::BWErr, run::RunLimits};
    let project = Project::new();
    let first = project.write("a.botwork", "Import |\"b.botwork\"| As |b|");
    let second = project.write("b.botwork", "Import |\"a.botwork\"| As |a|");
    let source = "Import |\"a.botwork\"| As |lib|";
    let baseline = project.run(source, &mut Context::default()).unwrap_err();
    let BWErr::ImportCycle(chain) = baseline.error.as_ref() else {
        panic!("cycle")
    };
    let a = botwork::core::paths::canonicalize(first).unwrap();
    let b = botwork::core::paths::canonicalize(second).unwrap();
    assert_eq!(
        chain,
        &format!("{} -> {} -> {}", a.display(), b.display(), a.display())
    );
    assert_eq!(baseline.related.len(), 3);
    let size = DiagnosticLimits::default().check(&baseline).unwrap();
    let exact = DiagnosticLimits {
        diagnostics: size.diagnostics,
        depth: size.depth,
        call_frames: size.call_frames,
        related_locations: size.related_locations,
        text_bytes: size.text_bytes,
        source_bytes: size.source_bytes,
    };
    let mut context = Context::with_limits(RunLimits {
        diagnostics: exact.clone(),
        ..RunLimits::default()
    })
    .unwrap();
    let error = project.run(source, &mut context).unwrap_err();
    assert_eq!(
        error.to_value().to_string(),
        baseline.to_value().to_string()
    );
    let mut context = Context::with_limits(RunLimits {
        diagnostics: DiagnosticLimits {
            text_bytes: 0,
            ..exact
        },
        ..RunLimits::default()
    })
    .unwrap();
    let error = project.run(source, &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
    assert_eq!(error.causes[0].code(), DiagnosticCode::ImportCycle);
    assert_eq!(
        error.causes[0]
            .omissions
            .as_ref()
            .unwrap()
            .related_locations,
        3
    );
    assert!(context.complete_statements("lib::").is_empty());
}

#[test]
fn imported_parse_execution_and_budget_failures_receive_the_import_site_once() {
    use botwork::core::run::{ImportLimits, RunLimits};
    let project = Project::new();
    project.write("syntax.botwork", "|x| = |");
    project.write("execution.botwork", "|x| = |unknown|");
    project.write("empty.botwork", "");
    for (file, limits) in [
        ("syntax.botwork", RunLimits::default()),
        ("execution.botwork", RunLimits::default()),
        (
            "empty.botwork",
            RunLimits {
                imports: ImportLimits {
                    loads: 0,
                    ..ImportLimits::default()
                },
                ..RunLimits::default()
            },
        ),
        (
            "empty.botwork",
            RunLimits {
                import_depth: 0,
                ..RunLimits::default()
            },
        ),
    ] {
        let mut context = Context::with_limits(limits).unwrap();
        let source = format!("Import |\"{file}\"| As |lib|");
        let error = project.run(&source, &mut context).unwrap_err();
        assert_eq!(error.related.len(), 1);
        assert_eq!(error.related[0].message, "imported here");
        assert_eq!(error.related[0].span.text(), source);
        assert!(context.complete_statements("lib::").is_empty());
    }
}

// macOS file systems store names as UTF-8 and refuse other bytes.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_canonical_import_paths_preserve_bounded_read_evidence_and_do_not_publish() {
    use botwork::core::{diagnostic::DiagnosticLimits, grammar::BWErr, run::RunLimits};
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let project = Project::new();
    let target = project
        .0
        .workspace
        .join(std::ffi::OsString::from_vec(b"bad\xff.botwork".to_vec()));
    fs::write(&target, "Value { Return |1| }").unwrap();
    symlink(target, project.0.workspace.join("alias.botwork")).unwrap();
    let source = "Import |\"alias.botwork\"| As |lib|";
    for text_bytes in [0, DiagnosticLimits::default().text_bytes] {
        let mut context = Context::with_limits(RunLimits {
            diagnostics: DiagnosticLimits {
                text_bytes,
                ..DiagnosticLimits::default()
            },
            ..RunLimits::default()
        })
        .unwrap();
        let error = project.run(source, &mut context).unwrap_err();
        if text_bytes == 0 {
            assert_eq!(error.code(), DiagnosticCode::ResourceLimit);
            assert_eq!(error.causes[0].code(), DiagnosticCode::ImportRead);
            assert_eq!(
                error.causes[0]
                    .omissions
                    .as_ref()
                    .unwrap()
                    .related_locations,
                1
            );
        } else {
            assert_eq!(error.code(), DiagnosticCode::ImportRead);
            let BWErr::ImportRead(message) = error.error.as_ref() else {
                panic!("import read")
            };
            assert_eq!(message, "Module paths must be valid UTF-8");
            assert_eq!(error.related.len(), 1);
            assert_eq!(error.span.as_ref().unwrap().text(), "\"alias.botwork\"");
        }
        assert!(context.complete_statements("lib::").is_empty());
    }
}

#[test]
fn invalid_utf8_contents_and_directory_paths_are_loading_errors_without_namespace_publication() {
    let project = Project::new();
    let path = project.write("invalid.botwork", "");
    fs::write(path, [0xff, 0xfe]).unwrap();
    fs::create_dir(project.0.workspace.join("directory.botwork")).unwrap();
    let mut context = Context::default();
    for path in ["invalid.botwork", "directory.botwork"] {
        let error = project
            .run(&format!("Import |\"{path}\"| As |lib|"), &mut context)
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ImportRead);
        assert_eq!(error.related.len(), 1);
    }
    project.write("invalid.botwork", "Value { Return |7| }");
    assert!(matches!(
        project.run(
            "Import |\"invalid.botwork\"| As |lib|\nlib::Value",
            &mut context
        ),
        Ok(Literal::Int(7))
    ));
}

#[test]
fn imported_handler_causes_survive_inspection_and_rethrow_in_the_importer() {
    let project = Project::new();
    project.write(
        "module.botwork",
        "Fail { Try { |x| = |1 / 0| } Catch { Return |missing| } }",
    );
    let mut context = Context::default();
    let error = project.run("Import |\"module.botwork\"| As |lib|\nTry { lib::Fail } Catch |error| { |observed| = |error.causes[0].code|\nRethrow }", &mut context).unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert_eq!(error.causes.len(), 1);
    assert_eq!(error.causes[0].code(), DiagnosticCode::Arithmetic);
    assert_eq!(error.call_stack[0].signature, "lib::fail");
    assert!(error.causes[0]
        .span
        .as_ref()
        .unwrap()
        .source()
        .name()
        .ends_with("module.botwork"));
    assert_eq!(
        project
            .run("|result| = |observed|", &mut context)
            .unwrap()
            .to_string(),
        "BW3002"
    );
}

#[test]
fn empty_modules_reserve_a_namespace_and_fresh_contexts_have_independent_caches() {
    let project = Project::new();
    project.write("empty.botwork", "");
    let mut context = Context::default();
    project
        .run("Import |\"empty.botwork\"| As |empty|", &mut context)
        .unwrap();
    assert!(context.complete_statements("empty::").is_empty());
    assert_eq!(
        project
            .run("empty::New {}", &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::DuplicateNamespace
    );
    project.write("empty.botwork", "Value { Return |7| }");
    assert!(matches!(
        project.run(
            "Import |\"empty.botwork\"| As |empty|\nempty::Value",
            &mut Context::default()
        ),
        Ok(Literal::Int(7))
    ));
}

#[test]
fn parser_pair_imports_support_absolute_paths_and_keep_the_input_import_site() {
    use botwork::core::{
        eval::botwork_detailed,
        grammar::{BWParser, Rule},
    };
    use pest::Parser;
    let project = Project::new();
    let path = project.write("module.botwork", "Value { Return |7| }");
    let path = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let source = format!("Import |\"{path}\"| As |lib|");
    let pair = BWParser::parse(Rule::stmt_import, &source)
        .unwrap()
        .next()
        .unwrap();
    let mut context = Context::default();
    assert!(matches!(
        botwork_detailed(pair, &mut context),
        Ok(Literal::None)
    ));
    assert!(matches!(
        project.run("lib::Value", &mut context),
        Ok(Literal::Int(7))
    ));
    let error = project.run("lib::New {}", &mut context).unwrap_err();
    assert_eq!(error.related[0].span.source().name(), "<input>");
}

#[test]
fn successful_dependencies_remain_cached_when_a_parent_module_fails_initialization() {
    let project = Project::new();
    project.write("child.botwork", "Initialize\nValue { Return |7| }");
    project.write(
        "parent.botwork",
        "Import |\"child.botwork\"| As |child|\n|x| = |missing|",
    );
    let count = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let mut context = Context::default();
    context
        .register_native("Initialize", move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    assert!(project
        .run("Import |\"parent.botwork\"| As |parent|", &mut context)
        .is_err());
    assert!(matches!(
        project.run(
            "Import |\"child.botwork\"| As |child|\nchild::Value",
            &mut context
        ),
        Ok(Literal::Int(7))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn imports_inside_exported_calls_use_definition_paths_and_cached_dependencies() {
    let project = Project::new();
    project.write("nested/dependency.botwork", "Value { Return |7| }");
    project.write(
        "nested/outer.botwork",
        "Read { Import |\"dependency.botwork\"| As |dep|\nReturn |@{dep::Value}| }",
    );
    let mut context = Context::default();
    assert_eq!(project.run("Import |\"nested/outer.botwork\"| As |lib|\n|result| = |[@{lib::Read}, @{lib::Read}]|", &mut context).unwrap().to_string(), "[7, 7]");
    assert!(context.statement_signature("dep::Value").unwrap().is_none());
}

#[test]
fn dropping_a_context_releases_cached_module_sources_without_reference_cycles() {
    let project = Project::new();
    project.write("child.botwork", "Value { Return |7| }");
    project.write(
        "parent.botwork",
        "Import |\"child.botwork\"| As |child|\nRead { Return |@{child::Value}| }",
    );
    let mut context = Context::default();
    project
        .run("Import |\"parent.botwork\"| As |lib|", &mut context)
        .unwrap();
    let parent = Arc::downgrade(
        context
            .statement_signature("lib::Read")
            .unwrap()
            .unwrap()
            .header()
            .source(),
    );
    let child = Arc::downgrade(
        context
            .statement_signature("lib::child::Value")
            .unwrap()
            .unwrap()
            .header()
            .source(),
    );
    drop(context);
    assert!(parent.upgrade().is_none());
    assert!(child.upgrade().is_none());
}

#[cfg(unix)]
#[test]
fn symlink_aliases_share_initialization_and_canonical_cycle_detection() {
    let project = Project::new();
    let path = project.write("module.botwork", "Import |\"alias.botwork\"| As |again|");
    std::os::unix::fs::symlink(&path, project.0.workspace.join("alias.botwork")).unwrap();
    assert_eq!(
        project
            .run(
                "Import |\"module.botwork\"| As |lib|",
                &mut Context::default()
            )
            .unwrap_err()
            .code(),
        DiagnosticCode::ImportCycle
    );
    project.write("module.botwork", "Value { Return |7| }");
    let value = project.run("Import |\"module.botwork\"| As |first|\nImport |\"alias.botwork\"| As |second|\n|result| = |@{first::Value} + @{second::Value}|", &mut Context::default()).unwrap();
    assert!(matches!(value, Literal::Int(14)));
}
impl Project {
    fn new() -> Self {
        Self(Harness::new())
    }
    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.0.workspace.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        path
    }
    fn run(&self, source: &str, context: &mut Context) -> DiagnosticResult<Literal> {
        let path = self.0.workspace.join("main.botwork");
        evaluate_program_detailed(
            &Program::parse_detailed(&path.to_string_lossy(), source)?,
            context,
        )
    }
}

#[test]
fn relative_imports_resolve_from_each_importer_and_support_qualified_composition() {
    let project = Project::new();
    project.write(
        "nested/math.botwork",
        "Import |\"../base.botwork\"| As |base|\nDouble |x| { Return |@{base::Value} * x| }",
    );
    project.write("base.botwork", "Value { Return |2| }");
    let result = project.run("Import |\"nested/math.botwork\"| As |Math|\n|answer| = |@{m a t h::Double |3|} + @{Math::base::Value}|", &mut Context::default()).unwrap();
    assert!(matches!(result, Literal::Int(8)));
}

#[test]
fn module_globals_and_helpers_are_isolated_from_the_importing_caller() {
    let project = Project::new();
    project.write(
        "library.botwork",
        "|value| = |10|\nHelper |x| { Return |x + value| }\nRead |x| { Return |@{Helper |x|}| }",
    );
    let mut context = Context::default();
    let result = project.run("|value| = |99|\nHelper |x| { Return |999| }\nImport |\"library.botwork\"| As |lib|\n|answer| = |[@{lib::Read |2|}, value]|", &mut context).unwrap();
    assert_eq!(result.to_string(), "[12, 99]");
    project.write("private.botwork", "Read { Return |secret| }");
    let error = project
        .run(
            "|secret| = |7|\nImport |\"private.botwork\"| As |private|\nprivate::Read",
            &mut context,
        )
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert!(error
        .span
        .as_ref()
        .unwrap()
        .source()
        .name()
        .ends_with("private.botwork"));
}

#[test]
fn modules_import_what_they_use_and_never_see_their_importers_imports() {
    use botwork::core::run::{Engine, RunOptions};
    let project = Project::new();
    project.write(
        "pages.botwork",
        "Import |\"botwork:webdriver\"| As |web|\nReady { Return |\"ready\"| }",
    );
    project.write(
        "leaky.botwork",
        "Leak |browser| { Return |@{ web::Title Of |browser| }| }",
    );
    let run = |source: &str| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(Engine::default().run_source_async(
            "main.botwork",
            source,
            RunOptions {
                working_directory: Some(project.0.workspace.clone()),
                ..RunOptions::default()
            },
        ))
    };
    // A module may import a module its importer imported, under the same name.
    let result = run("Import |\"botwork:webdriver\"| As |web|\nImport |\"pages.botwork\"| As |pages|\n|answer| = pages::Ready");
    assert!(result.result.is_ok(), "{:?}", result.result);
    assert_eq!(result.variables["answer"].to_string(), "ready");
    // A module that does not import it cannot reach its importer's.
    let result = run("Import |\"botwork:webdriver\"| As |web|\nImport |\"leaky.botwork\"| As |leaky|\nleaky::Leak |{session: \"s\"}|");
    let error = result.result.unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedStatement, "{error}");
    assert!(
        error
            .span
            .as_ref()
            .unwrap()
            .source()
            .name()
            .ends_with("leaky.botwork"),
        "{error}"
    );
}

#[test]
fn canonical_cache_initializes_once_across_aliases_and_retains_successful_snapshots() {
    let project = Project::new();
    let path = project.write("module.botwork", "Initialize\nValue { Return |7| }");
    fs::create_dir_all(project.0.workspace.join("nested")).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let mut context = Context::default();
    context
        .register_native("Initialize", move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    project.run("Import |\"module.botwork\"| As |first|\nImport |\"nested/../module.botwork\"| As |second|", &mut context).unwrap();
    fs::write(&path, "Value { Return |99| }").unwrap();
    assert!(matches!(
        project.run(
            "Import |\"module.botwork\"| As |third|\nthird::Value",
            &mut context
        ),
        Ok(Literal::Int(7))
    ));
    fs::remove_file(path).unwrap();
    assert!(matches!(
        project.run(
            "Import |\"module.botwork\"| As |fourth|\nfourth::Value",
            &mut context
        ),
        Ok(Literal::Int(7))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let mut cloned = context.clone();
    assert!(matches!(
        project.run(
            "Import |\"module.botwork\"| As |fifth|\nfifth::Value",
            &mut cloned
        ),
        Ok(Literal::Int(7))
    ));
    assert!(context
        .statement_signature("fifth::Value")
        .unwrap()
        .is_none());
}

#[test]
fn failed_initialization_publishes_no_namespace_and_can_be_retried() {
    let project = Project::new();
    let path = project.write("module.botwork", "Value { Return |1| }\n|x| = |missing|");
    let mut context = Context::default();
    let error = project
        .run("Import |\"module.botwork\"| As |lib|", &mut context)
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert!(context.statement_signature("lib::Value").unwrap().is_none());
    fs::write(path, "Value { Return |2| }").unwrap();
    assert!(matches!(
        project.run(
            "Import |\"module.botwork\"| As |lib|\nlib::Value",
            &mut context
        ),
        Ok(Literal::Int(2))
    ));
}

#[test]
fn direct_and_indirect_cycles_retain_the_chain_and_all_import_locations() {
    for indirect in [false, true] {
        let project = Project::new();
        project.write(
            "a.botwork",
            if indirect {
                "Import |\"b.botwork\"| As |b|"
            } else {
                "Import |\"a.botwork\"| As |self|"
            },
        );
        project.write("b.botwork", "Import |\"a.botwork\"| As |a|");
        let mut context = Context::default();
        let error = project
            .run("Import |\"a.botwork\"| As |lib|", &mut context)
            .unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::ImportCycle);
        assert!(error.to_string().contains("a.botwork ->"));
        assert_eq!(error.related.len(), if indirect { 3 } else { 2 });
        project.write("a.botwork", "Value { Return |7| }");
        assert!(matches!(
            project.run("Import |\"a.botwork\"| As |lib|\nlib::Value", &mut context),
            Ok(Literal::Int(7))
        ));
    }
}

#[test]
fn duplicate_aliases_and_existing_qualified_definitions_fail_before_initialization() {
    let project = Project::new();
    project.write("module.botwork", "Initialize\nValue { Return |7| }");
    let count = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let mut context = Context::default();
    context
        .register_native("Initialize", move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Literal::None)
        })
        .unwrap();
    project.run("lib::Existing {}", &mut context).unwrap();
    assert_eq!(
        project
            .run("Import |\"module.botwork\"| As |LIB|", &mut context)
            .unwrap_err()
            .code(),
        DiagnosticCode::DuplicateNamespace
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    project
        .run("Import |\"module.botwork\"| As |other|", &mut context)
        .unwrap();
    for source in [
        "Import |\"missing.botwork\"| As |OTHER|",
        "other::New {}",
        "other::Value {}",
    ] {
        let error = project.run(source, &mut context).unwrap_err();
        assert_eq!(error.code(), DiagnosticCode::DuplicateNamespace);
        assert_eq!(error.related.len(), 1);
    }
    assert!(matches!(
        project.run("other::Value", &mut context),
        Ok(Literal::Int(7))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn namespace_shadowing_hides_all_parent_exports_and_is_removed_after_invocation() {
    let project = Project::new();
    project.write(
        "outer.botwork",
        "Value { Return |1| }\nOnly outer { Return |3| }",
    );
    project.write("inner.botwork", "Value { Return |2| }");
    let mut context = Context::default();
    let value = project.run("Import |\"outer.botwork\"| As |lib|\nRun { Import |\"inner.botwork\"| As |lib|\nTry { lib::Only outer } Catch { Return |@{lib::Value}| } }\n|result| = |[@{Run}, @{lib::Value}, @{lib::Only outer}]|", &mut context).unwrap();
    assert_eq!(value.to_string(), "[2, 1, 3]");
}

#[test]
fn imported_call_failures_keep_original_frames_and_import_related_sites() {
    let project = Project::new();
    project.write(
        "module.botwork",
        "Inner { Return |missing| }\nOuter { Return |@{Inner}| }",
    );
    let error = project
        .run(
            "Import |\"module.botwork\"| As |lib|\nCaller { Return |@{lib::Outer}| }\nCaller",
            &mut Context::default(),
        )
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::UndefinedVariable);
    assert!(error
        .span
        .as_ref()
        .unwrap()
        .source()
        .name()
        .ends_with("module.botwork"));
    assert_eq!(
        error
            .call_stack
            .iter()
            .map(|frame| frame.signature.as_str())
            .collect::<Vec<_>>(),
        ["inner", "lib::outer", "caller"]
    );
    assert!(error
        .related
        .iter()
        .any(|related| related.message == "imported here"));
}

#[test]
fn malformed_modules_retain_their_syntax_error_and_do_not_execute_earlier_statements() {
    let project = Project::new();
    project.write("invalid.botwork", "Log |\"unreachable\"|\nReturn");
    let result = project
        .0
        .run(
            "main",
            "Try { Import |\"invalid.botwork\"| As |lib| } Catch |error| { Log |error.code| }",
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(result.stdout, b"BW1002\n");
    assert!(result.stderr.is_empty());
}

#[test]
fn import_paths_aliases_and_keywords_follow_explicit_syntax_rules() {
    let project = Project::new();
    project.write("தமிழ்.botwork", "வணக்கம் { Return |\"உலகம்\"| }");
    let mut context = Context::default();
    assert_eq!(
        project
            .run(
                "iMpOrT\n|\"தமிழ்.botwork\"|\nAs\n|மொழி|\nமொழி::வணக்கம்",
                &mut context
            )
            .unwrap()
            .to_string(),
        "உலகம்"
    );
    for source in [
        "Import |path| As |lib|",
        "Import |\"module.botwork\"|",
        "Import |\"module.botwork\"| As |1|",
        "Import |\"module.botwork\"| As |a-b|",
        "Import |\"module.botwork\"| As |true|",
    ] {
        assert_eq!(
            Program::parse_detailed("invalid.botwork", source)
                .unwrap_err()
                .code(),
            DiagnosticCode::Syntax
        );
    }
    for path in [
        "missing.botwork",
        "file.txt",
        "https://example.invalid/module.botwork",
    ] {
        let source = format!("Import |\"{path}\"| As |missing|");
        assert_eq!(
            project.run(&source, &mut context).unwrap_err().code(),
            DiagnosticCode::ImportRead
        );
    }
    assert!(matches!(
        project.run(
            "Importantly { Return |7| }\nAs { Return |8| }\nImportantly",
            &mut context
        ),
        Ok(Literal::Int(7))
    ));
}

#[test]
fn imported_metadata_uses_qualified_labels_and_original_definition_spans() {
    let project = Project::new();
    project.write("module.botwork", "Double |x| { Return |2 * x| }");
    let mut context = Context::default();
    project
        .run("Import |\"module.botwork\"| As |Math|", &mut context)
        .unwrap();
    let metadata = context
        .statement_signature("math::Double |value|")
        .unwrap()
        .unwrap();
    assert_eq!(metadata.display_header(), "Math::Double |x|");
    assert!(metadata.help().starts_with("Math::Double |x|"));
    assert!(metadata
        .header()
        .source()
        .name()
        .ends_with("module.botwork"));
    assert_eq!(context.complete_statements("MATH::D").len(), 1);
}

#[test]
fn cli_imports_initialize_once_and_use_source_paths_outside_the_process_directory() {
    let project = Project::new();
    project.write(
        "modules/math.botwork",
        "Log |\"initialize\"|\nDouble |x| { Return |x * 2| }",
    );
    let output = project.0.run("main", "Import |\"modules/math.botwork\"| As |first|\nImport |\"modules/math.botwork\"| As |second|\nLog |@{first::Double |3|} + @{second::Double |4|}|", Duration::from_secs(5)).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"initialize\n14\n");
    assert!(output.stderr.is_empty());
    let other_directory = project.0.workspace.join("unrelated");
    fs::create_dir_all(&other_directory).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"))
        .current_dir(other_directory)
        .arg("--file")
        .arg(project.0.workspace.join("main.botwork"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"initialize\n14\n");
}

#[test]
fn call_frames_show_statements_as_written_under_the_namespaces_used() {
    let project = Project::new();
    project.write(
        "nested/pricing.botwork",
        "Line total of |quantity| \\\n    at |price| {\n    Return |quantity * missing|\n}",
    );
    project.write(
        "shop.botwork",
        "Import |\"nested/pricing.botwork\"| As |pricing|",
    );
    let mut context = Context::default();
    context.init_statements();
    let error = project
        .run(
            "Import |\"shop.botwork\"| As |shop|\n|total| = |@{ shop::pricing::Line total of |3| at |4| }|",
            &mut context,
        )
        .unwrap_err();
    // One line, as written, under both namespaces; the signature is unchanged.
    let rendered = error.to_string();
    assert!(
        rendered.contains("in `shop::pricing::Line total of |quantity| at |price|` called at"),
        "{rendered}"
    );
    assert_eq!(
        error.call_stack[0].signature,
        "shop::pricing::linetotalof|param|at|param|"
    );
}

/// Python modules need a build with the `python` feature; elsewhere the import
/// says so, before running anything.
#[cfg(not(feature = "python"))]
#[test]
fn python_modules_need_a_python_build() {
    let project = Project::new();
    project.write(
        "helpers.py",
        "import botwork\n\n@botwork.statement(\"Greet\")\ndef greet():\n    return 1\n",
    );
    let error = project
        .run("Import |\"helpers.py\"| As |py|", &mut Context::default())
        .unwrap_err();
    assert_eq!(error.code(), DiagnosticCode::ImportRead);
    assert!(
        error
            .to_string()
            .contains("`helpers.py` is a Python module, which needs a Botwork build with the `python` feature"),
        "{error}"
    );
}
