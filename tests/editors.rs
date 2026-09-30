//! The editor packages in `editors/`: Vim, VS Code, and Helix. Vim and VS Code
//! highlight the shared fixtures in `editors/test` as `highlighting.json`
//! expects; Helix builds the Tree-sitter grammars and finds their queries; and
//! each editor starts the language server with `botwork --lsp`.
//!
//! A check that needs an editor, Node.js, or the Git history is skipped when it
//! is missing, unless `BOTWORK_REQUIRE_EDITORS` names it (`vim`, `vscode`,
//! `helix`, `git`) or is `all`, as in continuous integration.
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

/// Whether the check for `tool` may be skipped; panics when it is required.
fn skip(tool: &str, reason: &str) -> bool {
    let required = std::env::var("BOTWORK_REQUIRE_EDITORS").unwrap_or_default();
    let required = required
        .split(',')
        .any(|name| name.trim() == tool || name.trim() == "all");
    assert!(
        !required,
        "BOTWORK_REQUIRE_EDITORS requires {tool}, but {reason}"
    );
    eprintln!("skipped: {reason}");
    true
}

fn runs(program: &str, argument: &str) -> bool {
    Command::new(program)
        .arg(argument)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// One expected highlight: the token after `prefix` on `line` is `category`.
struct Expected {
    file: String,
    line: usize,
    prefix: String,
    token: String,
    category: String,
}

fn expectations() -> Vec<Expected> {
    let text = fs::read_to_string(root().join("editors/test/highlighting.json")).unwrap();
    let files: BTreeMap<String, Vec<(usize, String, String, String)>> =
        serde_json::from_str(&text).unwrap();
    files
        .into_iter()
        .flat_map(|(file, expected)| {
            expected
                .into_iter()
                .map(move |(line, prefix, token, category)| Expected {
                    file: file.clone(),
                    line,
                    prefix,
                    token,
                    category,
                })
        })
        .collect()
}

/// Vim's syntax groups at each byte of each line of a fixture, and the
/// buffer's filetype and comment string.
fn vim_groups(file: &Path) -> (Vec<Vec<String>>, String, String) {
    let probe = tempfile::tempdir().unwrap();
    let output = probe.path().join("groups");
    let script = probe.path().join("probe.vim");
    fs::write(
        &script,
        r#"let s:out = [&filetype, &commentstring]
for s:line in range(1, line('$'))
  let s:names = []
  for s:column in range(1, max([col([s:line, '$']) - 1, 0]))
    call add(s:names, synIDattr(synID(s:line, s:column, 1), 'name'))
  endfor
  call add(s:out, join(s:names, ' '))
endfor
call writefile(s:out, $PROBE_OUT)
qall!
"#,
    )
    .unwrap();
    let status = Command::new("vim")
        .args(["-Nu", "NONE", "-i", "NONE", "-es", "--cmd"])
        .arg(format!("set rtp^={}", root().join("editors/vim").display()))
        .args(["-c", "filetype plugin on", "-c", "syntax on", "-c"])
        .arg(format!("edit {}", file.display()))
        .arg("-c")
        .arg(format!("source {}", script.display()))
        .env("PROBE_OUT", &output)
        .status()
        .unwrap();
    assert!(status.success(), "vim failed");
    let text = fs::read_to_string(&output).unwrap();
    let mut lines = text.lines();
    let filetype = lines.next().unwrap().to_owned();
    let comments = lines.next().unwrap().to_owned();
    let groups = lines
        .map(|line| line.split(' ').map(str::to_owned).collect())
        .collect();
    (groups, filetype, comments)
}

/// Vim groups for each highlighting category.
fn vim_category(category: &str) -> &'static [&'static str] {
    match category {
        "comment" => &["botworkComment", "botworkBlockComment"],
        "string" => &["botworkString"],
        "escape" => &["botworkEscape"],
        "keyword" => &[
            "botworkKeyword",
            "botworkSuiteKeyword",
            "botworkHeaderKeyword",
        ],
        "number" => &["botworkNumber"],
        "boolean" => &["botworkBoolean"],
        "function" => &["botworkWords"],
        "namespace" => &["botworkQualifier"],
        "variable" => &["botworkVariable"],
        "operator" => &["botworkOperator", "botworkWordOperator"],
        "continuation" => &["botworkContinuation"],
        other => panic!("unknown category {other}"),
    }
}

#[test]
fn vim_highlights_the_shared_fixtures() {
    if !runs("vim", "--version") && skip("vim", "vim is not installed") {
        return;
    }
    let mut failures = Vec::new();
    let mut groups = BTreeMap::new();
    for expected in expectations() {
        let file = root().join("editors/test").join(&expected.file);
        let (lines, filetype, comments) = groups
            .entry(expected.file.clone())
            .or_insert_with(|| vim_groups(&file));
        assert_eq!((filetype.as_str(), comments.as_str()), ("botwork", "# %s"));
        let text = fs::read_to_string(&file).unwrap();
        let line = text.lines().nth(expected.line - 1).unwrap();
        let start = line
            .find(&format!("{}{}", expected.prefix, expected.token))
            .unwrap_or_else(|| panic!("{}:{}: {:?}", expected.file, expected.line, expected.token))
            + expected.prefix.len();
        let allowed = vim_category(&expected.category);
        for (offset, _) in expected.token.char_indices() {
            let group = &lines[expected.line - 1][start + offset];
            if !allowed.contains(&group.as_str()) {
                failures.push(format!(
                    "{}:{}:{}: {:?} is {group:?}, expected {}",
                    expected.file,
                    expected.line,
                    start + offset + 1,
                    expected.token,
                    expected.category
                ));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root().join(path)).unwrap()).unwrap()
}

#[test]
fn the_vscode_manifest_describes_both_languages_and_the_server() {
    let manifest = json("editors/vscode/package.json");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest["main"], "./extension.js");
    let languages = manifest["contributes"]["languages"].as_array().unwrap();
    let ids: Vec<&str> = languages
        .iter()
        .map(|language| language["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["botwork", "botwork-suite"]);
    assert_eq!(languages[0]["extensions"], serde_json::json!([".botwork"]));
    assert_eq!(
        languages[1]["filenamePatterns"],
        serde_json::json!(["*.suite.botwork", "*.dataset.botwork"])
    );
    for language in languages {
        let configuration = language["configuration"].as_str().unwrap();
        let configuration = json(&format!("editors/vscode/{configuration}"));
        assert_eq!(configuration["comments"]["lineComment"], "#");
        assert_eq!(
            configuration["comments"]["blockComment"],
            serde_json::json!(["###", "###"])
        );
    }
    for grammar in manifest["contributes"]["grammars"].as_array().unwrap() {
        let path = grammar["path"].as_str().unwrap();
        let file = json(&format!("editors/vscode/{path}"));
        assert_eq!(file["scopeName"], grammar["scopeName"], "{path}");
        assert!(ids.contains(&grammar["language"].as_str().unwrap()));
    }
    let setting = &manifest["contributes"]["configuration"]["properties"]["botwork.server.path"];
    assert_eq!(setting["default"], "botwork");
    let extension = fs::read_to_string(root().join("editors/vscode/extension.js")).unwrap();
    assert!(extension.contains(r#"args: ["--lsp"]"#));
    // The lockfile pins exactly the manifest's dependencies, and the language
    // client's supported VS Code versions are the extension's.
    let lock = json("editors/vscode/package-lock.json");
    for kind in ["dependencies", "devDependencies"] {
        assert_eq!(lock["packages"][""][kind], manifest[kind], "{kind}");
    }
    assert_eq!(
        lock["packages"]["node_modules/vscode-languageclient"]["engines"]["vscode"],
        manifest["engines"]["vscode"]
    );
}

#[test]
fn the_vscode_extension_activates_and_highlights_the_shared_fixtures() {
    let directory = root().join("editors/vscode");
    if !runs("node", "--version") && skip("vscode", "Node.js is not installed") {
        return;
    }
    if !directory.join("node_modules").is_dir()
        && skip("vscode", "run `npm ci` in editors/vscode first")
    {
        return;
    }
    for test in ["test/extension.test.js", "test/grammar.test.js"] {
        let output = Command::new("node")
            .arg(test)
            .current_dir(&directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{test}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn helix_queries_are_the_tree_sitter_queries() {
    let queries = root().join("editors/tree-sitter-botwork/queries");
    let helix = root().join("editors/helix/queries");
    assert_eq!(
        fs::read_to_string(helix.join("botwork/highlights.scm")).unwrap(),
        fs::read_to_string(queries.join("highlights.scm")).unwrap(),
        "copy editors/tree-sitter-botwork/queries/highlights.scm to editors/helix/queries/botwork/"
    );
    assert_eq!(
        fs::read_to_string(helix.join("botwork-suite/highlights.scm")).unwrap(),
        format!(
            "; inherits: botwork\n{}",
            fs::read_to_string(queries.join("suite-highlights.scm")).unwrap()
        ),
        "editors/helix/queries/botwork-suite/highlights.scm inherits the script queries and adds the suite queries"
    );
}

/// The revision `languages.toml` pins both grammars to.
fn pinned_revision(languages: &str) -> String {
    let revisions: Vec<&str> = languages
        .lines()
        .filter_map(|line| line.split_once("rev = \"")?.1.split_once('"'))
        .map(|(revision, _)| revision)
        .collect();
    assert_eq!(revisions.len(), 2, "{languages}");
    assert_eq!(revisions[0], revisions[1]);
    assert_eq!(revisions[0].len(), 40);
    revisions[0].to_owned()
}

#[test]
fn helix_fetches_the_current_grammars() {
    let languages = fs::read_to_string(root().join("editors/helix/languages.toml")).unwrap();
    let revision = pinned_revision(&languages);
    for expected in [
        r#"command = "botwork""#,
        r#"args = ["--lsp"]"#,
        r#"grammar = "botwork""#,
        r#"grammar = "botwork_suite""#,
        r#"file-types = [{ glob = "*.suite.botwork" }, { glob = "*.dataset.botwork" }]"#,
    ] {
        assert!(languages.contains(expected), "{expected}");
    }
    let tree = |revision: &str, path: &str| {
        Command::new("git")
            .args(["rev-parse", &format!("{revision}:{path}")])
            .current_dir(root())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    for grammar in ["botwork", "suite"] {
        let path = format!("editors/tree-sitter-botwork/{grammar}");
        let Some(pinned) = tree(&revision, &path) else {
            if skip(
                "git",
                "the pinned grammar revision is not in this checkout's history",
            ) {
                return;
            }
            unreachable!();
        };
        // The pinned grammar is the one checked in, and it is not modified.
        assert_eq!(
            Some(pinned),
            tree("HEAD", &path),
            "{path} changed after {revision}; update the pinned revision"
        );
        let clean = Command::new("git")
            .args(["diff", "--quiet", "HEAD", "--", &path])
            .current_dir(root())
            .status()
            .unwrap();
        assert!(clean.success(), "{path} has uncommitted changes");
    }
}

#[test]
fn helix_builds_the_grammars_and_finds_the_queries_and_server() {
    if !runs("hx", "--version") && skip("helix", "Helix is not installed") {
        return;
    }
    let config = tempfile::tempdir().unwrap();
    let helix = config.path().join("helix");
    fs::create_dir_all(helix.join("runtime/grammars")).unwrap();
    copy(
        &root().join("editors/helix/queries"),
        &helix.join("runtime/queries"),
    );
    // Build from this checkout rather than fetching the pinned revision.
    let languages = fs::read_to_string(root().join("editors/helix/languages.toml")).unwrap();
    let local: String = languages
        .lines()
        .map(|line| match line.split_once("subpath = \"") {
            Some((_, rest)) if line.starts_with("source") => {
                let subpath = rest.split_once('"').unwrap().0;
                format!(
                    "source = {{ path = \"{}\" }}",
                    root().join(subpath).display()
                )
            }
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        helix.join("languages.toml"),
        format!("use-grammars = {{ only = [\"botwork\", \"botwork_suite\"] }}\n{local}\n"),
    )
    .unwrap();
    // Helix treats a Cargo manifest directory as its own source checkout.
    let build = Command::new("hx")
        .args(["--grammar", "build"])
        .env("XDG_CONFIG_HOME", config.path())
        .env_remove("CARGO_MANIFEST_DIR")
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let binary = Path::new(env!("CARGO_BIN_EXE_botwork")).parent().unwrap();
    let path = std::env::join_paths(std::iter::once(binary.to_owned()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .unwrap();
    for language in ["botwork", "botwork-suite"] {
        let health = Command::new("hx")
            .args(["--health", language])
            .env("XDG_CONFIG_HOME", config.path())
            .env_remove("CARGO_MANIFEST_DIR")
            .env("PATH", &path)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&health.stdout);
        // Strip colors, which some versions print regardless.
        let text: String = text
            .split('\u{1b}')
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    part
                } else {
                    part.split_once('m').map_or(part, |(_, rest)| rest)
                }
            })
            .collect();
        for expected in [
            "Tree-sitter parser: ✓",
            "Highlight queries: ✓",
            "✓ botwork: ",
        ] {
            assert!(text.contains(expected), "{language}: {expected}\n{text}");
        }
    }
}

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
