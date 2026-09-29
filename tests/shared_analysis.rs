//! `--check` and the language server share one analysis: for identical input
//! they report the same problems, with the same codes, severities, files, and
//! source ranges. `--check` counts columns in Unicode scalar values and the
//! server in UTF-16 code units, so both are compared as byte ranges.
#[path = "support/lsp_client.rs"]
mod lsp_client;
#[path = "support/sources.rs"]
mod sources;

use botwork::core::format::SourceKind;
use lsp_client::{file_uri, Client};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::LazyLock,
};

/// A problem: its file, code (or rule), severity, and byte range.
type Problem = (PathBuf, String, &'static str, usize, usize);

static HEADLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?<file>\S.*?\.botwork):(?<line>\d+):(?<column>\d+)(?:-(?<end_line>\d+):(?<end_column>\d+))?: (?:(?<severity>error|warning)\[(?<rule>[a-z-]+)\]: )?(?:\[(?<code>BW\d{4})\] )?",
    )
    .unwrap()
});

fn text_of(path: &Path, documents: &BTreeMap<PathBuf, String>) -> String {
    documents
        .get(path)
        .cloned()
        .unwrap_or_else(|| fs::read_to_string(path).unwrap())
}

/// The byte offset of a one-based line and Unicode scalar column.
fn scalar_offset(text: &str, line: usize, column: usize) -> usize {
    let start: usize = text
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum();
    start
        + text[start..]
            .chars()
            .take(column - 1)
            .map(char::len_utf8)
            .sum::<usize>()
}

/// The byte offset of a zero-based line and UTF-16 character.
fn utf16_offset(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap() as usize;
    let mut units = position["character"].as_u64().unwrap() as usize;
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let mut offset = start;
    for character in text[start..].chars() {
        if units == 0 {
            break;
        }
        assert!(units >= character.len_utf16(), "inside a surrogate pair");
        units -= character.len_utf16();
        offset += character.len_utf8();
    }
    offset
}

/// What `botwork --check` reports for a file or suite, run from `directory`.
fn check(
    directory: &Path,
    path: &Path,
    suite: bool,
    documents: &BTreeMap<PathBuf, String>,
) -> Vec<Problem> {
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .arg("--check")
        .arg(if suite { "--suite" } else { "--file" })
        .arg(path)
        .current_dir(directory)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let mut found = Vec::new();
    for line in stderr.lines() {
        let Some(headline) = HEADLINE.captures(line) else {
            assert!(
                !line.starts_with(&path.display().to_string()) || line.starts_with(' '),
                "unparsed headline: {line}"
            );
            continue;
        };
        let file = fs::canonicalize(directory.join(&headline["file"])).unwrap();
        let text = text_of(&file, documents);
        let number = |name: &str| {
            headline
                .name(name)
                .map(|value| value.as_str().parse().unwrap())
        };
        let start = scalar_offset(&text, number("line").unwrap(), number("column").unwrap());
        let end = match (number("end_line"), number("end_column")) {
            (Some(line), Some(column)) => scalar_offset(&text, line, column),
            _ => start,
        };
        let code = headline
            .name("code")
            .or(headline.name("rule"))
            .expect("a code or a rule")
            .as_str()
            .to_owned();
        let severity = match headline.name("severity").map(|value| value.as_str()) {
            Some("warning") => "warning",
            _ => "error",
        };
        found.push((file, code, severity, start, end));
    }
    found.sort();
    found
}

/// The language server's current diagnostics across every file it published for.
struct Published {
    latest: BTreeMap<String, Vec<Value>>,
}

impl Published {
    /// Read publications until `uri`'s own, which the server sends last.
    fn read(&mut self, client: &mut Client, uri: &str) {
        loop {
            let message = client.receive();
            if message["method"] != "textDocument/publishDiagnostics" {
                continue;
            }
            let published = message["params"]["uri"].as_str().unwrap().to_owned();
            let diagnostics = message["params"]["diagnostics"].as_array().unwrap().clone();
            self.latest.insert(published.clone(), diagnostics);
            if published == uri {
                return;
            }
        }
    }

    fn problems(&self, documents: &BTreeMap<PathBuf, String>) -> Vec<Problem> {
        let mut found = Vec::new();
        for (uri, diagnostics) in &self.latest {
            if diagnostics.is_empty() {
                continue;
            }
            let path = url::Url::parse(uri).unwrap().to_file_path().unwrap();
            let file = fs::canonicalize(&path).unwrap();
            let text = text_of(&file, documents);
            for diagnostic in diagnostics {
                let start = utf16_offset(&text, &diagnostic["range"]["start"]);
                let end = utf16_offset(&text, &diagnostic["range"]["end"]);
                let severity = match diagnostic["severity"].as_i64() {
                    Some(1) => "error",
                    Some(2) => "warning",
                    other => panic!("severity {other:?}"),
                };
                let code = diagnostic["code"].as_str().unwrap().to_owned();
                found.push((file.clone(), code, severity, start, end));
            }
        }
        found.sort();
        found
    }
}

impl Published {
    /// Open `path` with `text`, require the server's problems to equal
    /// `--check`'s, then close it and require every diagnostic to clear.
    fn agree(
        &mut self,
        client: &mut Client,
        directory: &Path,
        path: &Path,
        text: &str,
        suite: bool,
    ) -> Vec<Problem> {
        let documents = BTreeMap::from([(path.to_owned(), text.to_owned())]);
        let expected = check(directory, path, suite, &documents);
        let uri = file_uri(path);
        client.notify(
            "textDocument/didOpen",
            serde_json::json!({"textDocument": {"uri": uri, "languageId": "botwork", "version": 1, "text": text}}),
        );
        self.read(client, &uri);
        assert_eq!(
            self.problems(&documents),
            expected,
            "{}\n{text}",
            path.display()
        );
        // Closing clears the document and every module it reached.
        client.notify(
            "textDocument/didClose",
            serde_json::json!({"textDocument": {"uri": uri}}),
        );
        self.read(client, &uri);
        assert_eq!(
            self.problems(&documents),
            [],
            "{}: stale diagnostics",
            path.display()
        );
        expected
    }
}

#[test]
fn check_and_the_language_server_agree_across_the_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = tempfile::tempdir().unwrap();
    let mut client = Client::start(root);
    let mut published = Published {
        latest: BTreeMap::new(),
    };
    let (mut compared, mut with_problems) = (0, 0);
    for (index, (origin, text, kind)) in sources::sources().into_iter().enumerate() {
        let suite = match kind {
            SourceKind::Script => false,
            SourceKind::Suite => true,
            // `--check` does not take datasets.
            SourceKind::Dataset => continue,
        };
        // Files are checked in place, so their imports resolve; other sources
        // are written to a file of their own.
        let path = if Path::new(&origin).is_file() {
            PathBuf::from(&origin)
        } else {
            let extension = if suite { "suite.botwork" } else { "botwork" };
            let path = workspace.path().join(format!("source{index}.{extension}"));
            fs::write(&path, &text).unwrap();
            path
        };
        let path = fs::canonicalize(path).unwrap();
        let expected = published.agree(&mut client, root, &path, &text, suite);
        compared += 1;
        with_problems += usize::from(!expected.is_empty());
    }
    assert!(
        compared > 200 && with_problems > 50,
        "{compared} {with_problems}"
    );
    assert_eq!(client.finish(true), Some(0));
}

/// Every prefix of a script, as if typed from the start, and then the whole
/// script again after each character is deleted from the end.
#[test]
fn every_incomplete_edit_reports_what_check_reports() {
    let directory = tempfile::tempdir().unwrap();
    let directory = fs::canonicalize(directory.path()).unwrap();
    fs::create_dir(directory.join("lib")).unwrap();
    // The module's warning sits after characters that take two UTF-16 units.
    fs::write(
        directory.join("lib/m.botwork"),
        "# 😀 ü\nDouble |x| { Return |x * y| }\n",
    )
    .unwrap();
    let script = "Import |\"lib/m.botwork\"| As |m|\n|s| = |\"😀é\"| # ✓\nLog |@{ m::Double |2| }|\nLog |[\"😀\", @{ Missing |s| }]|\nIf |1| { Log |ü| }\nShow |v| {\n    Return |v|\n    Log |s|\n}\n";
    let path = directory.join("main.botwork");
    let uri = file_uri(&path);
    let mut client = Client::start(&directory);
    let mut published = Published {
        latest: BTreeMap::new(),
    };
    client.notify(
        "textDocument/didOpen",
        serde_json::json!({"textDocument": {"uri": uri, "languageId": "botwork", "version": 0, "text": ""}}),
    );
    published.read(&mut client, &uri);
    let mut ends: Vec<usize> = script.char_indices().map(|(index, _)| index).collect();
    ends.push(script.len());
    let backwards: Vec<usize> = ends.iter().rev().copied().collect();
    let (mut version, mut in_module, mut syntax) = (0, 0, 0);
    for end in ends.into_iter().chain(backwards) {
        let text = &script[..end];
        fs::write(&path, text).unwrap();
        let documents = BTreeMap::from([(path.clone(), text.to_owned())]);
        let expected = check(&directory, &path, false, &documents);
        version += 1;
        client.notify(
            "textDocument/didChange",
            serde_json::json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        );
        published.read(&mut client, &uri);
        assert_eq!(published.problems(&documents), expected, "{text:?}");
        in_module += usize::from(expected.iter().any(|problem| problem.0 != path));
        syntax += usize::from(expected.iter().any(|problem| problem.1 == "BW1001"));
    }
    // The module's warning came and went as the import was typed and broken.
    assert!(in_module > 50 && syntax > 50, "{in_module} {syntax}");
    fs::write(&path, script).unwrap();
    let whole = BTreeMap::from([(path.clone(), script.to_owned())]);
    let codes: Vec<_> = check(&directory, &path, false, &whole)
        .into_iter()
        .map(|problem| problem.1)
        .collect();
    for code in ["BW2001", "BW2002", "BW3003", "unreachable-code"] {
        assert!(codes.iter().any(|found| found == code), "{code}: {codes:?}");
    }
    assert_eq!(client.finish(true), Some(0));
}

#[test]
fn imported_modules_are_reported_alike() {
    let directory = tempfile::tempdir().unwrap();
    let directory = fs::canonicalize(directory.path()).unwrap();
    fs::create_dir(directory.join("lib")).unwrap();
    for (file, text) in [
        ("lib/bad.botwork", "# ü\nLog |1\n"),
        ("lib/math.botwork", "# 😀\nDouble |x| { Return |x * y| }\n"),
        ("lib/wrap.botwork", "Import |\"math.botwork\"| As |m|\n"),
        ("lib/a.botwork", "Import |\"b.botwork\"| As |b|\n"),
        ("lib/b.botwork", "Import |\"a.botwork\"| As |a|\n"),
    ] {
        fs::write(directory.join(file), text).unwrap();
    }
    let mut client = Client::start(&directory);
    let mut published = Published {
        latest: BTreeMap::new(),
    };
    let path = directory.join("main.botwork");
    let scenarios: [(&str, &[&str]); 7] = [
        // A module that does not parse.
        ("Import |\"lib/bad.botwork\"| As |bad|\n", &["BW1001"]),
        // Missing and non-local modules.
        (
            "Import |\"lib/none.botwork\"| As |n|\nImport |\"https://x/m.botwork\"| As |r|\n",
            &["BW6001", "BW6001"],
        ),
        // A cycle through two modules.
        ("Import |\"lib/a.botwork\"| As |a|\n", &["BW6002"]),
        // One alias for two modules.
        (
            "Import |\"lib/math.botwork\"| As |m|\nImport |\"lib/wrap.botwork\"| As |m|\n",
            &["BW2001", "BW6003"],
        ),
        // A module's warning, reached through a re-export.
        (
            "Import |\"lib/wrap.botwork\"| As |w|\nLog |@{ w::m::Double |2| }|\n",
            &["BW2001"],
        ),
        // Calls a module does not define, directly and through a re-export.
        (
            "Import |\"lib/wrap.botwork\"| As |w|\nLog |@{ w::Double |2| }|\nLog |@{ w::m::Triple |2| }|\n",
            &["BW2001", "BW2002", "BW2002"],
        ),
        // A call before its import runs.
        (
            "Log |@{ m::Double |2| }|\nImport |\"lib/math.botwork\"| As |m|\n",
            &["BW2001", "BW2002"],
        ),
    ];
    for (text, codes) in scenarios {
        fs::write(&path, text).unwrap();
        let problems = published.agree(&mut client, &directory, &path, text, false);
        let mut found: Vec<&str> = problems.iter().map(|problem| problem.1.as_str()).collect();
        found.sort();
        assert_eq!(found, codes, "{text}");
    }
    assert_eq!(client.finish(true), Some(0));
}

#[test]
fn syntax_errors_are_placed_alike() {
    let directory = tempfile::tempdir().unwrap();
    let directory = fs::canonicalize(directory.path()).unwrap();
    let mut client = Client::start(&directory);
    let mut published = Published {
        latest: BTreeMap::new(),
    };
    let path = directory.join("main.botwork");
    // Errors in the middle of the text, some after characters that take two
    // UTF-16 code units.
    for text in [
        "Log |\"😀\" +|\nLog |2|\n",
        "Log |(1|\nLog |2|\n",
        "Log |{a 1}|\n# ü\n",
        "|x| = |\"é😀\"|\nIf |x|\nLog |1|\n",
        "Log |a.|\nLog |\"😀\"|\n",
        "Log |\"😀\"| {\nLog |1|\n",
        "Else { Log |\"😀\"| }\n",
    ] {
        fs::write(&path, text).unwrap();
        let problems = published.agree(&mut client, &directory, &path, text, false);
        assert!(
            problems
                .iter()
                .any(|problem| problem.1 == "BW1001" && problem.3 < text.len()),
            "{text:?}: {problems:?}"
        );
    }
    assert_eq!(client.finish(true), Some(0));
}
