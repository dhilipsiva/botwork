//! Renaming preserves resolution and behavior across the language corpus.
//! Every definition and variable in each conformance script, and every
//! definition in the example modules, is renamed. Each rename is either
//! refused with a reason, or its result must resolve every call and variable
//! as before and run with the same output, exit status, and error codes.
#[path = "support/cli_harness.rs"]
mod cli_harness;
#[path = "conformance/cases.rs"]
#[allow(dead_code)]
mod corpus;

use botwork::core::{
    ast::{Block, ElseBranch, Name, Program, Statement, StatementKind},
    format::SourceKind,
    language::{apply, Language},
};
use cli_harness::Harness;
use corpus::Input;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
    time::Duration,
};

/// A symbol to rename: its offset and whether it is a definition.
type Symbol = (usize, bool);

struct Symbols {
    found: Vec<Symbol>,
    names: BTreeSet<String>,
}

impl Symbols {
    /// The first binding of each variable name.
    fn bind(&mut self, name: &Name) {
        if self.names.insert(name.text.clone()) {
            self.found.push((name.span.start(), false));
        }
    }

    fn blocks(&mut self, blocks: &[&Block]) {
        for block in blocks {
            self.statements(&block.statements);
        }
    }

    /// Every definition header and variable binding, nested blocks included.
    fn statements(&mut self, statements: &[Statement]) {
        for statement in statements {
            match statement.kind() {
                StatementKind::Define(definition) => {
                    self.found.push((definition.header.start(), true));
                    for parameter in &definition.parameters {
                        self.bind(parameter);
                    }
                    self.blocks(&[&definition.body]);
                }
                StatementKind::Assign { name, .. } => self.bind(name),
                StatementKind::For { binding, body, .. } => {
                    self.bind(binding);
                    self.blocks(&[body]);
                }
                StatementKind::Try {
                    body,
                    binding,
                    handler,
                } => {
                    if let Some(binding) = binding {
                        self.bind(binding);
                    }
                    self.blocks(&[body, handler]);
                }
                StatementKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    self.blocks(&[then_branch]);
                    match else_branch {
                        Some(ElseBranch::Block(block)) => self.blocks(&[block]),
                        Some(ElseBranch::If(nested)) => {
                            self.statements(std::slice::from_ref(nested.as_ref()))
                        }
                        None => {}
                    }
                }
                StatementKind::While { body, .. } | StatementKind::Poll { body, .. } => {
                    self.blocks(&[body])
                }
                StatementKind::Finally { body, cleanup } => self.blocks(&[body, cleanup]),
                _ => {}
            }
        }
    }
}

/// A new name: a fresh variable name, or a header with a fresh first word.
fn fresh(placeholder: &str, definition: bool, count: usize) -> String {
    match placeholder.find('|') {
        _ if !definition => format!("renamed_{count}"),
        Some(pipe) => format!("Renamed{count} {}", &placeholder[pipe..]),
        None => format!("Renamed{count}"),
    }
}

/// What a run shows: stdout, the exit status, and the diagnostic codes.
fn outcome(output: std::process::Output) -> (String, Option<i32>, BTreeSet<String>) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let codes = stderr
        .match_indices("[BW")
        .map(|(at, _)| stderr[at + 1..at + 7].to_owned())
        .collect();
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.code(),
        codes,
    )
}

/// Reasons a rename may be refused; any other refusal fails the test.
const REASONS: [&str; 9] = [
    "is already defined in this file",
    "is already called in this file",
    "is already a variable in this file",
    "enclosing or nested scope",
    "inside a block",
    "looked up by name",
    "input variable",
    "same scope",
    "is a built-in statement",
];

#[test]
fn renames_in_the_conformance_corpus_keep_resolution_and_behavior() {
    let harness = Harness::new();
    let language = Language::new(&harness.workspace);
    let (mut renamed, mut refused) = (0, BTreeMap::<&str, usize>::new());
    let (mut definitions, mut variables) = (0, 0);
    for case in corpus::cases() {
        let Input::Script(source) = case.input else {
            continue;
        };
        let name = harness.workspace.join(format!("{}.botwork", case.id));
        let name = name.to_string_lossy().into_owned();
        let analysis = language.analyze(&name, source, SourceKind::Script);
        let Ok(program) = Program::parse_detailed(&name, source) else {
            continue;
        };
        if !analysis.parsed {
            continue;
        }
        let mut symbols = Symbols {
            found: Vec::new(),
            names: BTreeSet::new(),
        };
        symbols.statements(&program.statements);
        let mut original = None;
        for (offset, definition) in symbols.found {
            let Ok(target) = analysis.prepare_rename(offset) else {
                continue;
            };
            let new_name = fresh(&target.placeholder, definition, renamed);
            let documents = BTreeMap::from([(name.clone(), source.to_owned())]);
            let edits = match language.rename(&documents, &name, offset, &new_name) {
                Ok(edits) => edits,
                Err(reason) => {
                    let known = REASONS
                        .iter()
                        .find(|known| reason.contains(**known))
                        .unwrap_or_else(|| panic!("{}: {reason}\n{source}", case.id));
                    *refused.entry(known).or_default() += 1;
                    continue;
                }
            };
            let text = apply(source, edits.get(&name).map_or(&[], Vec::as_slice));
            assert!(edits.keys().all(|file| *file == name), "{edits:?}");
            let after = language.analyze(&name, &text, SourceKind::Script);
            assert!(after.parsed, "{}: {new_name}\n{text}", case.id);
            assert_eq!(
                after.resolution(),
                analysis.resolution(),
                "{}: {new_name}\n{source}\n{text}",
                case.id
            );
            let original = original.get_or_insert_with(|| {
                outcome(
                    harness
                        .run(case.id, source, Duration::from_secs(10))
                        .unwrap(),
                )
            });
            let run = outcome(
                harness
                    .run(case.id, &text, Duration::from_secs(10))
                    .unwrap(),
            );
            assert_eq!(&run, original, "{}: {new_name}\n{text}", case.id);
            renamed += 1;
            if definition {
                definitions += 1;
            } else {
                variables += 1;
            }
        }
    }
    eprintln!(
        "renamed {renamed} ({definitions} definitions, {variables} variables); refused {refused:?}"
    );
    assert!(
        definitions >= 30 && variables >= 80,
        "{definitions} {variables} {refused:?}"
    );
}

#[test]
fn renames_in_the_examples_keep_resolution_and_behavior() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    copy(&examples, &root);
    let language = Language::new(&root);
    let mut scripts: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| SourceKind::of_path(path) == SourceKind::Script)
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "botwork")
        })
        .collect();
    scripts.sort();
    let (mut renamed, mut compared, mut refused) = (0, 0, BTreeMap::<&str, usize>::new());
    for path in scripts {
        let name = path.to_string_lossy().into_owned();
        let source = fs::read_to_string(&path).unwrap();
        let analysis = language.analyze(&name, &source, SourceKind::Script);
        let program = Program::parse_detailed(&name, &source).unwrap();
        let mut symbols = Symbols {
            found: Vec::new(),
            names: BTreeSet::new(),
        };
        symbols.statements(&program.statements);
        // Output that differs between two runs is not compared.
        let original = run_example(&root, &path);
        let deterministic = run_example(&root, &path) == original;
        for (offset, definition) in symbols.found {
            let Ok(target) = analysis.prepare_rename(offset) else {
                continue;
            };
            let new_name = fresh(&target.placeholder, definition, renamed);
            let documents = BTreeMap::from([(name.clone(), source.clone())]);
            let edits = match language.rename(&documents, &name, offset, &new_name) {
                Ok(edits) => edits,
                Err(reason) => {
                    let known = REASONS
                        .iter()
                        .find(|known| reason.contains(**known))
                        .unwrap_or_else(|| panic!("{name}: {reason}"));
                    *refused.entry(known).or_default() += 1;
                    continue;
                }
            };
            let text = apply(&source, edits.get(&name).map_or(&[], Vec::as_slice));
            let after = language.analyze(&name, &text, SourceKind::Script);
            assert_eq!(
                after.resolution(),
                analysis.resolution(),
                "{name}: {new_name}\n{text}"
            );
            if deterministic {
                fs::write(&path, &text).unwrap();
                assert_eq!(
                    run_example(&root, &path),
                    original,
                    "{name}: {new_name}\n{text}"
                );
                fs::write(&path, &source).unwrap();
                compared += 1;
            }
            renamed += 1;
        }
    }
    eprintln!("renamed {renamed}, ran {compared}; refused {refused:?}");
    assert!(
        renamed >= 100 && compared >= 80,
        "{renamed} {compared} {refused:?}"
    );
}

/// Copy a directory tree.
fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn run_example(directory: &Path, file: &Path) -> (String, Option<i32>, BTreeSet<String>) {
    outcome(
        Command::new(env!("CARGO_BIN_EXE_botwork"))
            .arg("--file")
            .arg(file)
            .current_dir(directory)
            .output()
            .unwrap(),
    )
}

#[test]
fn renaming_module_definitions_updates_importers_and_keeps_their_behavior() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let modules: Vec<_> = {
        let mut modules: Vec<_> = fs::read_dir(examples.join("modules"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "botwork")
            })
            .collect();
        modules.sort();
        modules
    };
    let (mut renamed, mut importers) = (0, BTreeSet::new());
    for module in &modules {
        let source = fs::read_to_string(module).unwrap();
        let Ok(program) = Program::parse_detailed("module", &source) else {
            continue;
        };
        let headers: Vec<usize> = program
            .statements
            .iter()
            .filter_map(|statement| match statement.kind() {
                StatementKind::Define(definition) => Some(definition.header.start()),
                _ => None,
            })
            .collect();
        for offset in headers {
            let workspace = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(workspace.path()).unwrap();
            copy(&examples, &root);
            let mut documents = BTreeMap::new();
            for directory in [root.clone(), root.join("modules")] {
                for entry in fs::read_dir(&directory).unwrap() {
                    let path = entry.unwrap().path();
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "botwork")
                    {
                        documents.insert(
                            path.to_string_lossy().into_owned(),
                            fs::read_to_string(&path).unwrap(),
                        );
                    }
                }
            }
            let name = root
                .join("modules")
                .join(module.file_name().unwrap())
                .to_string_lossy()
                .into_owned();
            let language = Language::new(&root);
            let placeholder = language
                .analyze(&name, &documents[&name], SourceKind::Script)
                .prepare_rename(offset)
                .unwrap()
                .placeholder;
            let new_name = fresh(&placeholder, true, renamed);
            let edits = language
                .rename(&documents, &name, offset, &new_name)
                .unwrap_or_else(|reason| panic!("{name}: {placeholder}: {reason}"));
            // Behavior before, for every importer the rename edits.
            let changed: Vec<String> = edits
                .keys()
                .filter(|file| !file.contains("/modules/"))
                .cloned()
                .collect();
            let before: Vec<_> = changed
                .iter()
                .map(|file| run_example(&root, Path::new(file)))
                .collect();
            for (file, edits) in &edits {
                fs::write(file, apply(&documents[file], edits)).unwrap();
            }
            for (file, before) in changed.iter().zip(before) {
                assert_eq!(
                    run_example(&root, Path::new(file)),
                    before,
                    "{placeholder} -> {new_name} in {file}"
                );
                importers.insert(Path::new(file).file_name().unwrap().to_owned());
            }
            renamed += 1;
        }
    }
    assert!(
        renamed >= 3 && !importers.is_empty(),
        "{renamed} {importers:?}"
    );
}
