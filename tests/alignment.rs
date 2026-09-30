//! Highlighting, parsing, formatting, and editor analysis stay aligned over one
//! shared corpus: every source in `tests/support/sources.rs`, which includes
//! the editor fixtures, and the formatted form of each.
//!
//! - The three highlighters (the Tree-sitter queries, Vim, and VS Code's
//!   TextMate grammars) classify every character of every valid source alike as
//!   a comment, string, number, keyword, or none of those.
//! - Formatted sources still parse in Tree-sitter, and the language analysis
//!   reports the same problems for them.
//! - On invalid input, each tool recovers as `docs/alignment.md` documents.
//!
//! The checks need the Tree-sitter CLI, Vim, and the VS Code extension's
//! dependencies (`npm ci` in `editors/vscode`). Each is skipped when missing,
//! unless `BOTWORK_REQUIRE_EDITORS` names it (`tree-sitter`, `vim`, `vscode`)
//! or is `all`, as in continuous integration.
#[path = "support/sources.rs"]
mod sources;

use botwork::core::{
    ast::{
        suite::{Dataset, Suite},
        Program,
    },
    format::{format, SourceKind},
    language::Language,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_owned()
}

/// Whether `tool` is available; panics when it is required but missing.
fn available(tool: &str, found: bool) -> bool {
    let required = std::env::var("BOTWORK_REQUIRE_EDITORS").unwrap_or_default();
    let required = required
        .split(',')
        .any(|name| name.trim() == tool || name.trim() == "all");
    assert!(
        found || !required,
        "BOTWORK_REQUIRE_EDITORS requires {tool}"
    );
    if !found {
        eprintln!("skipped: {tool} is not available");
    }
    found
}

fn runs(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn tools() -> bool {
    let tree_sitter = available("tree-sitter", runs("tree-sitter"));
    let vim = available("vim", runs("vim"));
    let vscode = available(
        "vscode",
        runs("node") && root().join("editors/vscode/node_modules").is_dir(),
    );
    tree_sitter && vim && vscode
}

/// What a highlighter says a character is, among what the tools must agree on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Category {
    Comment,
    String,
    Number,
    Keyword,
    Other,
}

fn parses(source: &str, kind: SourceKind) -> bool {
    match kind {
        SourceKind::Script => Program::parse_detailed("source.botwork", source).is_ok(),
        SourceKind::Suite => Suite::parse("source.suite.botwork", source).is_ok(),
        SourceKind::Dataset => Dataset::parse("source.dataset.botwork", source).is_ok(),
    }
}

fn extension(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Script => "botwork",
        SourceKind::Suite => "suite.botwork",
        SourceKind::Dataset => "dataset.botwork",
    }
}

/// A source written to `path` for the tools, and where it came from.
struct Sample {
    origin: String,
    path: PathBuf,
    text: String,
    kind: SourceKind,
}

/// Every valid corpus source, and its formatted form when that differs.
fn samples(directory: &Path) -> Vec<Sample> {
    let mut samples = Vec::new();
    for (index, (origin, text, kind)) in sources::sources().into_iter().enumerate() {
        if !parses(&text, kind) {
            continue;
        }
        let formatted = format(&origin, &text, kind).unwrap();
        let mut variants = vec![(origin.clone(), text.clone())];
        if formatted != text {
            variants.push((format!("formatted {origin}"), formatted));
        }
        for (variant, (origin, text)) in variants.into_iter().enumerate() {
            let path = directory.join(format!("s{index}v{variant}.{}", extension(kind)));
            fs::write(&path, &text).unwrap();
            samples.push(Sample {
                origin,
                path,
                text,
                kind,
            });
        }
    }
    samples
}

/// A capture: start row and column, end row and column, and its name.
type Capture = (usize, usize, usize, usize, String);

/// The captures a query file makes, by file.
fn tree_sitter_captures(
    grammar: &str,
    query: &str,
    files: &[&Path],
) -> BTreeMap<PathBuf, Vec<Capture>> {
    let mut captures: BTreeMap<PathBuf, Vec<_>> = BTreeMap::new();
    if files.is_empty() {
        return captures;
    }
    let output = Command::new("tree-sitter")
        .arg("query")
        .arg(
            root()
                .join("editors/tree-sitter-botwork/queries")
                .join(query),
        )
        .args(files)
        .current_dir(root().join("editors/tree-sitter-botwork").join(grammar))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let mut current = None;
    for line in text.lines() {
        if !line.starts_with(' ') {
            current = Some(PathBuf::from(line.trim()));
            continue;
        }
        let Some((_, capture)) = line.split_once("capture: ") else {
            continue;
        };
        // `N - name, start: (row, column), end: (row, column), text: ...`, where
        // multi-line captures omit `N - `.
        let capture = capture.split_once(" - ").map_or(capture, |(_, rest)| rest);
        let (name, rest) = capture.split_once(", start: (").unwrap();
        let (start, rest) = rest.split_once("), end: (").unwrap();
        let (end, _) = rest.split_once(')').unwrap();
        let point = |text: &str| {
            let (row, column) = text.split_once(", ").unwrap();
            (
                row.parse::<usize>().unwrap(),
                column.parse::<usize>().unwrap(),
            )
        };
        let ((start_row, start_column), (end_row, end_column)) = (point(start), point(end));
        captures.entry(current.clone().unwrap()).or_default().push((
            start_row,
            start_column,
            end_row,
            end_column,
            name.to_owned(),
        ));
    }
    captures
}

/// The byte offset of each line's start.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

fn tree_sitter_category(name: &str) -> Category {
    match name.split('.').next().unwrap() {
        "comment" => Category::Comment,
        // Suite IDs are strings that the Tree-sitter queries label.
        "string" | "label" => Category::String,
        "number" => Category::Number,
        "keyword" => Category::Keyword,
        _ => Category::Other,
    }
}

/// Tree-sitter's category for each byte: the innermost capture wins.
fn tree_sitter_categories(samples: &[Sample]) -> BTreeMap<PathBuf, Vec<Category>> {
    let scripts: Vec<&Path> = samples
        .iter()
        .filter(|sample| sample.kind == SourceKind::Script)
        .map(|sample| sample.path.as_path())
        .collect();
    let suites: Vec<&Path> = samples
        .iter()
        .filter(|sample| sample.kind != SourceKind::Script)
        .map(|sample| sample.path.as_path())
        .collect();
    let mut captures = tree_sitter_captures("botwork", "highlights.scm", &scripts);
    for query in ["highlights.scm", "suite-highlights.scm"] {
        for (file, found) in tree_sitter_captures("suite", query, &suites) {
            captures.entry(file).or_default().extend(found);
        }
    }
    samples
        .iter()
        .map(|sample| {
            let starts = line_starts(&sample.text);
            let mut spans: Vec<(usize, usize, Category)> = captures
                .get(&sample.path)
                .into_iter()
                .flatten()
                .map(|(start_row, start_column, end_row, end_column, name)| {
                    (
                        starts[*start_row] + start_column,
                        (starts[*end_row] + end_column).min(sample.text.len()),
                        tree_sitter_category(name),
                    )
                })
                .collect();
            // Wider captures first, so that narrower ones overwrite them.
            spans.sort_by_key(|(start, end, _)| std::cmp::Reverse(end - start));
            let mut categories = vec![Category::Other; sample.text.len()];
            for (start, end, category) in spans {
                categories[start..end].fill(category);
            }
            (sample.path.clone(), categories)
        })
        .collect()
}

fn vim_category(group: &str) -> Category {
    match group {
        "botworkComment" | "botworkBlockComment" => Category::Comment,
        "botworkString" | "botworkEscape" => Category::String,
        "botworkNumber" => Category::Number,
        "botworkKeyword" | "botworkSuiteKeyword" | "botworkHeaderKeyword" => Category::Keyword,
        _ => Category::Other,
    }
}

/// Vim's category for each byte, from one headless Vim over every file.
fn vim_categories(samples: &[Sample], directory: &Path) -> BTreeMap<PathBuf, Vec<Category>> {
    let list = directory.join("vim-files");
    let files: Vec<String> = samples
        .iter()
        .map(|sample| sample.path.display().to_string())
        .collect();
    fs::write(&list, files.join("\n")).unwrap();
    let script = directory.join("probe.vim");
    fs::write(
        &script,
        r#"set hidden
for s:file in readfile($PROBE_LIST)
  execute 'edit ' . fnameescape(s:file)
  let s:out = []
  for s:line in range(1, line('$'))
    let s:names = []
    for s:column in range(1, max([col([s:line, '$']) - 1, 0]))
      call add(s:names, synIDattr(synID(s:line, s:column, 1), 'name'))
    endfor
    call add(s:out, join(s:names, ' '))
  endfor
  call writefile(s:out, s:file . '.groups')
endfor
qall!
"#,
    )
    .unwrap();
    let status = Command::new("vim")
        .args(["-Nu", "NONE", "-i", "NONE", "-es", "--cmd"])
        .arg(format!("set rtp^={}", root().join("editors/vim").display()))
        .args(["-c", "filetype plugin on", "-c", "syntax on", "-c"])
        .arg(format!("source {}", script.display()))
        .env("PROBE_LIST", &list)
        .status()
        .unwrap();
    assert!(status.success(), "vim failed");
    samples
        .iter()
        .map(|sample| {
            let groups = fs::read_to_string(format!("{}.groups", sample.path.display())).unwrap();
            let groups: Vec<&str> = groups.split('\n').collect();
            let mut categories = vec![Category::Other; sample.text.len()];
            for (row, (start, line)) in line_starts(&sample.text)
                .into_iter()
                .zip(sample.text.split('\n'))
                .enumerate()
            {
                let names: Vec<&str> = groups
                    .get(row)
                    .map_or(vec![], |row| row.split(' ').collect());
                for (column, category) in
                    categories[start..start + line.len()].iter_mut().enumerate()
                {
                    *category = vim_category(names.get(column).copied().unwrap_or(""));
                }
            }
            (sample.path.clone(), categories)
        })
        .collect()
}

fn textmate_category(scopes: &[Value]) -> Category {
    let scopes: Vec<&str> = scopes.iter().map(|scope| scope.as_str().unwrap()).collect();
    // The innermost scope, past the punctuation that opens a comment or string.
    let innermost = scopes
        .iter()
        .rev()
        .find(|scope| !scope.starts_with("punctuation.definition."))
        .copied()
        .unwrap_or("");
    let prefixed = |prefixes: &[&str]| prefixes.iter().any(|prefix| innermost.starts_with(prefix));
    if prefixed(&["comment."]) {
        Category::Comment
    } else if prefixed(&["string.", "constant.character.escape."]) {
        Category::String
    } else if prefixed(&["constant.numeric."]) {
        Category::Number
    } else if prefixed(&["keyword.control.", "keyword.other.suite."]) {
        Category::Keyword
    } else {
        Category::Other
    }
}

/// VS Code's category for each byte, from its TextMate engine.
fn textmate_categories(samples: &[Sample]) -> BTreeMap<PathBuf, Vec<Category>> {
    let output = Command::new("node")
        .arg("test/scopes.js")
        .args(samples.iter().map(|sample| &sample.path))
        .current_dir(root().join("editors/vscode"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut found = BTreeMap::new();
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        let path = PathBuf::from(record["file"].as_str().unwrap());
        let text = fs::read_to_string(&path).unwrap();
        let mut categories = vec![Category::Other; text.len()];
        for ((start, line), tokens) in line_starts(&text)
            .into_iter()
            .zip(text.split('\n'))
            .zip(record["lines"].as_array().unwrap())
        {
            // Token offsets count UTF-16 code units; map them to bytes.
            let mut bytes = vec![0];
            for character in line.chars() {
                let last = *bytes.last().unwrap();
                for _ in 0..character.len_utf16() {
                    bytes.push(last + character.len_utf8());
                }
            }
            for token in tokens.as_array().unwrap() {
                let units = |index: usize| {
                    bytes[token[index].as_u64().unwrap().min(bytes.len() as u64 - 1) as usize]
                };
                let category = textmate_category(token[2].as_array().unwrap());
                categories[start + units(0)..start + units(1)].fill(category);
            }
        }
        found.insert(path, categories);
    }
    found
}

#[test]
fn highlighters_classify_every_character_of_the_corpus_alike() {
    if !tools() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let samples = samples(directory.path());
    let tree_sitter = tree_sitter_categories(&samples);
    let vim = vim_categories(&samples, directory.path());
    let textmate = textmate_categories(&samples);
    let mut disagreements = BTreeMap::<(Category, Category, Category), Vec<String>>::new();
    let mut compared = BTreeMap::<Category, usize>::new();
    for sample in &samples {
        let (tree_sitter, vim, textmate) = (
            &tree_sitter[&sample.path],
            &vim[&sample.path],
            &textmate[&sample.path],
        );
        for (offset, character) in sample.text.char_indices() {
            if character.is_whitespace() {
                continue;
            }
            let categories = (tree_sitter[offset], vim[offset], textmate[offset]);
            *compared.entry(categories.0).or_default() += 1;
            if categories.0 != categories.1 || categories.0 != categories.2 {
                let line_start = sample.text[..offset].rfind('\n').map_or(0, |at| at + 1);
                let line_end = sample.text[offset..]
                    .find('\n')
                    .map_or(sample.text.len(), |at| offset + at);
                let examples = disagreements.entry(categories).or_default();
                if examples.len() < 3 {
                    examples.push(format!(
                        "{} at byte {}: {:?}",
                        sample.origin,
                        offset - line_start,
                        &sample.text[line_start..line_end]
                    ));
                }
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "(Tree-sitter, Vim, TextMate) disagree:\n{disagreements:#?}"
    );
    eprintln!(
        "compared {} sources, by Tree-sitter category: {compared:?}",
        samples.len()
    );
    // Every category occurs, in scripts and suites, so the comparison has teeth.
    assert!(samples.len() > 250, "{}", samples.len());
    assert!(samples
        .iter()
        .any(|sample| sample.kind == SourceKind::Suite));
    for category in [
        Category::Comment,
        Category::String,
        Category::Number,
        Category::Keyword,
    ] {
        assert!(
            compared.get(&category).copied().unwrap_or(0) > 500,
            "{category:?}: {compared:?}"
        );
    }
}

/// Files Tree-sitter parses with an error node.
fn tree_sitter_rejects(grammar: &str, files: &[&Path]) -> BTreeSet<PathBuf> {
    if files.is_empty() {
        return BTreeSet::new();
    }
    let list = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        list.path(),
        files
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let output = Command::new("tree-sitter")
        .args(["parse", "--quiet", "--paths"])
        .arg(list.path())
        .current_dir(root().join("editors/tree-sitter-botwork").join(grammar))
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split('\t').next())
        .map(|path| PathBuf::from(path.trim()))
        .collect()
}

#[test]
fn formatted_sources_parse_and_analyze_like_their_originals() {
    let directory = tempfile::tempdir().unwrap();
    let samples = samples(directory.path());
    let language = Language::new(root());
    let codes = |sample: &Sample, text: &str| {
        // Analyze under the original's name, so its imports resolve alike.
        let name = if Path::new(&sample.origin).is_file() {
            sample.origin.clone()
        } else {
            sample.path.display().to_string()
        };
        let analysis = language.analyze(&name, text, sample.kind);
        let mut codes: Vec<String> = analysis
            .problems
            .iter()
            .chain(analysis.related.values().flat_map(|file| &file.problems))
            .map(|problem| problem.code.clone())
            .collect();
        codes.sort();
        codes
    };
    let mut formatted = 0;
    let originals: BTreeMap<&str, &Sample> = samples
        .iter()
        .filter(|sample| !sample.origin.starts_with("formatted "))
        .map(|sample| (sample.origin.as_str(), sample))
        .collect();
    for sample in samples
        .iter()
        .filter(|sample| sample.origin.starts_with("formatted "))
    {
        let original = originals[&sample.origin["formatted ".len()..]];
        let analyzed = Sample {
            origin: original.origin.clone(),
            path: original.path.clone(),
            text: String::new(),
            kind: sample.kind,
        };
        assert_eq!(
            codes(&analyzed, &sample.text),
            codes(&analyzed, &original.text),
            "{}",
            sample.origin
        );
        formatted += 1;
    }
    assert!(formatted > 50, "{formatted}");
    if !available("tree-sitter", runs("tree-sitter")) {
        return;
    }
    for (grammar, kinds) in [
        ("botwork", &[SourceKind::Script][..]),
        ("suite", &[SourceKind::Suite, SourceKind::Dataset][..]),
    ] {
        let files: Vec<&Path> = samples
            .iter()
            .filter(|sample| kinds.contains(&sample.kind))
            .map(|sample| sample.path.as_path())
            .collect();
        let rejected = tree_sitter_rejects(grammar, &files);
        let rejected: Vec<&String> = samples
            .iter()
            .filter(|sample| rejected.contains(&sample.path))
            .map(|sample| &sample.origin)
            .collect();
        assert!(rejected.is_empty(), "Tree-sitter rejects {rejected:?}");
    }
}

#[test]
fn each_tool_recovers_from_syntax_errors_as_documented() {
    // Two syntax errors, on the first and fourth lines, around valid lines.
    let text = "Log |1 +|\n# still a comment\nLog |\"text\"|\nIf |x|\nLog |2|\n";
    // The interpreter, the analysis, and the formatter stop at the first error.
    let error = Program::parse_detailed("errors.botwork", text).unwrap_err();
    assert_eq!(error.code().to_string(), "BW1001");
    let first = error.span.as_ref().unwrap().start();
    assert!(first < text.find('\n').unwrap());
    let language = Language::new(root());
    let analysis = language.analyze("errors.botwork", text, SourceKind::Script);
    let problems: Vec<_> = analysis
        .problems
        .iter()
        .map(|problem| (problem.code.as_str(), problem.start))
        .collect();
    assert_eq!(problems, [("BW1001", first)]);
    let refused = format("errors.botwork", text, SourceKind::Script).unwrap_err();
    assert_eq!(refused.span.as_ref().map(|span| span.start()), Some(first));
    if !tools() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("errors.botwork");
    fs::write(&path, text).unwrap();
    // Tree-sitter recovers and marks both errors.
    let tree = Command::new("tree-sitter")
        .arg("parse")
        .arg(&path)
        .current_dir(root().join("editors/tree-sitter-botwork/botwork"))
        .output()
        .unwrap();
    let tree = String::from_utf8_lossy(&tree.stdout);
    let error_rows: BTreeSet<&str> = tree
        .match_indices("(ERROR [")
        .chain(tree.match_indices("(MISSING"))
        .filter_map(|(at, _)| {
            tree[at..]
                .split_once('[')?
                .1
                .split_once(',')
                .map(|(row, _)| row)
        })
        .collect();
    assert!(
        error_rows.contains("0") && error_rows.contains("3"),
        "{error_rows:?}\n{tree}"
    );
    // Every highlighter still highlights the valid lines between the errors.
    let sample = Sample {
        origin: "errors".into(),
        path: path.clone(),
        text: text.to_owned(),
        kind: SourceKind::Script,
    };
    let samples = [sample];
    let comment = text.find("# still").unwrap();
    let string = text.find("text").unwrap();
    for (tool, categories) in [
        ("Tree-sitter", tree_sitter_categories(&samples)),
        ("Vim", vim_categories(&samples, directory.path())),
        ("TextMate", textmate_categories(&samples)),
    ] {
        let categories = &categories[&path];
        assert_eq!(categories[comment], Category::Comment, "{tool}");
        assert_eq!(categories[string], Category::String, "{tool}");
    }
}
