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
