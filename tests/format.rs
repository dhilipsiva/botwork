//! Formatting every valid script, suite, and dataset in the conformance corpus,
//! the examples, and the executed documentation is idempotent, keeps every
//! comment and string literal, and leaves the syntax tree unchanged. Invalid
//! input is refused.
#[path = "support/sources.rs"]
mod sources;

use botwork::core::{
    ast::{
        suite::{Dataset, Suite},
        AccessSegment, AssignmentValue, Block, Call, ElseBranch, Expr, ExprKind, Program,
        Statement, StatementKind,
    },
    format::{format, SourceKind},
};
use sources::sources;
use std::{fmt::Write, fs, path::Path};

/// The source with each comment and string literal replaced by one letter.
fn code(source: &str) -> String {
    let (comments, strings) = tokens(source);
    let mut code = source.to_owned();
    for (token, letter) in comments
        .iter()
        .map(|comment| (comment, "C"))
        .chain(strings.iter().map(|string| (string, "S")))
    {
        code = code.replacen(token.as_str(), letter, 1);
    }
    code
}

/// Comments and string literals, in order, skipping string contents.
fn tokens(source: &str) -> (Vec<String>, Vec<String>) {
    let (mut comments, mut strings) = (Vec::new(), Vec::new());
    let mut index = 0;
    while index < source.len() {
        let rest = &source[index..];
        if rest.starts_with('"') {
            let mut end = 1;
            let bytes = rest.as_bytes();
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = (end + 1).min(rest.len());
            strings.push(rest[..end].to_owned());
            index += end;
        } else if let Some(body) = rest.strip_prefix("###") {
            let end = body.find("###").map_or(rest.len(), |close| close + 6);
            comments.push(rest[..end].to_owned());
            index += end;
        } else if rest.starts_with('#') {
            let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
            comments.push(rest[..end].to_owned());
            index += end;
        } else {
            index += rest.chars().next().unwrap().len_utf8();
        }
    }
    (comments, strings)
}

fn shape_block(block: &[Statement], shape: &mut String) {
    shape.push('{');
    for statement in block {
        shape_statement(statement, shape);
        shape.push(';');
    }
    shape.push('}');
}

fn shape_blocks(blocks: &[&Block], shape: &mut String) {
    for block in blocks {
        shape_block(&block.statements, shape);
    }
}

fn shape_statement(statement: &Statement, shape: &mut String) {
    match statement.kind() {
        StatementKind::Assign { name, value } => {
            write!(shape, "{}=", name.text).unwrap();
            match value {
                AssignmentValue::Expression(value) => shape_expression(value, shape),
                AssignmentValue::Call(call) => shape_call(call, shape),
            }
        }
        StatementKind::Define(definition) => {
            write!(shape, "def {}", definition.signature).unwrap();
            for parameter in &definition.parameters {
                write!(shape, " {}", parameter.text).unwrap();
            }
            shape_blocks(&[&definition.body], shape);
        }
        StatementKind::Invoke(call) => shape_call(call, shape),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            shape.push_str("if");
            shape_expression(condition, shape);
            shape_blocks(&[then_branch], shape);
            match else_branch {
                Some(ElseBranch::Block(block)) => shape_blocks(&[block], shape),
                Some(ElseBranch::If(nested)) => shape_statement(nested, shape),
                None => {}
            }
        }
        StatementKind::For {
            binding,
            iterable,
            body,
        } => {
            write!(shape, "for {}", binding.text).unwrap();
            shape_expression(iterable, shape);
            shape_blocks(&[body], shape);
        }
        StatementKind::While { condition, body } => {
            shape.push_str("while");
            shape_expression(condition, shape);
            shape_blocks(&[body], shape);
        }
        StatementKind::Poll {
            mode,
            options,
            body,
            ..
        } => {
            write!(shape, "{mode:?}").unwrap();
            shape_expression(options, shape);
            shape_blocks(&[body], shape);
        }
        StatementKind::Try {
            body,
            binding,
            handler,
        } => {
            write!(shape, "try {:?}", binding.as_ref().map(|name| &name.text)).unwrap();
            shape_blocks(&[body, handler], shape);
        }
        StatementKind::Finally { body, cleanup } => {
            shape.push_str("finally");
            shape_blocks(&[body, cleanup], shape);
        }
        StatementKind::Return(value) => {
            shape.push_str("return");
            if let Some(value) = value {
                shape_expression(value, shape);
            }
        }
        StatementKind::Import {
            path, namespace, ..
        } => write!(shape, "import {path} {}", namespace.text).unwrap(),
        other => write!(shape, "{}", statement_name(other)).unwrap(),
    }
}

fn statement_name(kind: &StatementKind) -> &'static str {
    match kind {
        StatementKind::Break => "break",
        StatementKind::Continue => "continue",
        _ => "rethrow",
    }
}

fn shape_call(call: &Call, shape: &mut String) {
    write!(shape, "{}(", call.signature).unwrap();
    for argument in &call.arguments {
        shape_expression(argument, shape);
        shape.push(',');
    }
    shape.push(')');
}

fn shape_expression(expression: &Expr, shape: &mut String) {
    match &expression.kind {
        ExprKind::Call(call) => shape_call(call, shape),
        ExprKind::Access { base, segments } => {
            shape_expression(base, shape);
            for segment in segments {
                match segment {
                    AccessSegment::Literal(name) => write!(shape, ".{}", name.text).unwrap(),
                    AccessSegment::Computed { index, .. } => {
                        shape.push('[');
                        shape_expression(index, shape);
                        shape.push(']');
                    }
                }
            }
        }
        ExprKind::Array(values) => {
            shape.push('[');
            for value in values {
                shape_expression(value, shape);
                shape.push(',');
            }
            shape.push(']');
        }
        ExprKind::Map(entries) => {
            shape.push('{');
            for (key, value) in entries {
                write!(shape, "{:?}:", key.text).unwrap();
                shape_expression(value, shape);
                shape.push(',');
            }
            shape.push('}');
        }
        ExprKind::Unary {
            operator, operand, ..
        } => {
            write!(shape, "({operator:?}").unwrap();
            shape_expression(operand, shape);
            shape.push(')');
        }
        ExprKind::Binary {
            operator,
            left,
            right,
            ..
        } => {
            write!(shape, "({operator:?}").unwrap();
            shape_expression(left, shape);
            shape.push(' ');
            shape_expression(right, shape);
            shape.push(')');
        }
        literal => write!(shape, "{literal:?}").unwrap(),
    }
}

/// What a source declares, independent of the library's own comparison.
fn meaning(name: &str, source: &str, kind: SourceKind) -> Option<String> {
    let mut shape = String::new();
    match kind {
        SourceKind::Script => {
            let program = Program::parse_detailed(name, source).ok()?;
            shape_block(&program.statements, &mut shape);
        }
        SourceKind::Suite => {
            let suite = Suite::parse(name, source).ok()?;
            write!(shape, "{:?}", suite.metadata()).unwrap();
            let fixtures = suite.fixture_programs();
            shape_block(&fixtures.setup.statements, &mut shape);
            shape_block(&fixtures.teardown.statements, &mut shape);
            for (index, case) in suite.cases().iter().enumerate() {
                write!(shape, "{:?}{:?}", case.metadata(), case.binding()).unwrap();
                shape_block(&suite.program(index).unwrap().statements, &mut shape);
            }
        }
        SourceKind::Dataset => {
            let dataset = Dataset::parse(name, source).ok()?;
            write!(shape, "{:?}", dataset.metadata()).unwrap();
            for row in dataset.rows() {
                write!(shape, "{:?}={}", row.metadata(), row.value()).unwrap();
            }
        }
    }
    Some(shape)
}

#[test]
fn formatting_is_idempotent_and_keeps_comments_literals_and_meaning() {
    let mut formatted_count = 0;
    let mut refused = Vec::new();
    for (name, source, kind) in sources() {
        let Some(before) = meaning("source.botwork", &source, kind) else {
            // Invalid input is refused without output.
            assert!(
                format(&name, &source, kind).is_err(),
                "{name}: invalid input was formatted"
            );
            refused.push(name);
            continue;
        };
        let once = format(&name, &source, kind).unwrap_or_else(|error| panic!("{name}: {error}"));
        let twice = format(&name, &once, kind).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(once, twice, "{name}: formatting is not idempotent");
        assert_eq!(
            meaning("source.botwork", &once, kind).as_deref(),
            Some(before.as_str()),
            "{name}: the syntax tree changed\n{once}"
        );
        assert_eq!(
            tokens(&once),
            tokens(&source),
            "{name}: comments or literals changed\n{once}"
        );
        assert!(!once.contains('\r') || source.contains('\r'), "{name}");
        assert!(once.is_empty() || once.ends_with('\n'), "{name}");
        assert!(
            code(&once).lines().all(|line| line == line.trim_end()),
            "{name}: trailing whitespace\n{once}"
        );
        formatted_count += 1;
    }
    assert!(formatted_count > 150, "{formatted_count} sources formatted");
    assert!(refused.len() >= 10, "{refused:?}");
}

fn cli(directory: &Path, arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn format_check_reports_without_writing_and_format_rewrites_in_place() {
    let directory = tempfile::tempdir().unwrap();
    let messy = "log|\"x\"|\nif|true|{Write File|\"ran.txt\"| Text|\"yes\"|}\n";
    fs::write(directory.path().join("messy.botwork"), messy).unwrap();
    fs::write(directory.path().join("clean.botwork"), "Log |1|\n").unwrap();
    let files = ["--file", "messy.botwork", "--file", "clean.botwork"];
    let (status, stdout, stderr) = cli(
        directory.path(),
        &[&["--format-check"][..], &files].concat(),
    );
    assert_eq!((status, stdout.as_str()), (Some(1), ""));
    assert_eq!(
        stderr,
        "would reformat messy.botwork\n[format] 2 files: 1 would change, 0 errors\n"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("messy.botwork")).unwrap(),
        messy
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            directory.path().join("messy.botwork"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
    }
    let (status, _, stderr) = cli(directory.path(), &[&["--format"][..], &files].concat());
    assert_eq!(status, Some(0), "{stderr}");
    assert_eq!(
        stderr,
        "reformatted messy.botwork\n[format] 2 files: 1 reformatted, 0 errors\n"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("messy.botwork")).unwrap(),
        "log |\"x\"|\nIf |true| {\n    Write File |\"ran.txt\"| Text |\"yes\"|\n}\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(directory.path().join("messy.botwork"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }
    // Formatting never runs the script, and leaves no temporary file behind.
    assert!(!directory.path().join("ran.txt").exists());
    let mut names: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["clean.botwork", "messy.botwork"]);
    let (status, _, stderr) = cli(
        directory.path(),
        &[&["--format-check"][..], &files].concat(),
    );
    assert_eq!(
        (status, stderr.as_str()),
        (Some(0), "[format] 2 files: 0 would change, 0 errors\n")
    );
}

#[test]
fn invalid_files_are_reported_and_left_untouched() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("bad.botwork"), "Log |1\n").unwrap();
    let (status, _, stderr) = cli(
        directory.path(),
        &[
            "--format",
            "--file",
            "bad.botwork",
            "--file",
            "missing.botwork",
        ],
    );
    assert_eq!(status, Some(1));
    assert!(stderr.contains("[BW1001]"), "{stderr}");
    assert!(stderr.contains("missing.botwork: "), "{stderr}");
    assert!(
        stderr.ends_with("[format] 2 files: 0 reformatted, 2 errors\n"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("bad.botwork")).unwrap(),
        "Log |1\n"
    );
}

#[test]
fn suites_and_datasets_are_recognized_by_name() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("s.suite.botwork"),
        "Suite |\"s\"| {\ncase |\"c\"| { log |1| }\n}\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("d.dataset.botwork"),
        "dataset |\"d\"| { row |\"r\"| values |1| }\n",
    )
    .unwrap();
    let (status, _, stderr) = cli(
        directory.path(),
        &[
            "--format",
            "--suite",
            "s.suite.botwork",
            "--suite",
            "d.dataset.botwork",
        ],
    );
    assert_eq!(status, Some(0), "{stderr}");
    assert_eq!(
        fs::read_to_string(directory.path().join("s.suite.botwork")).unwrap(),
        "Suite |\"s\"| {\n    Case |\"c\"| {\n        log |1|\n    }\n}\n"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("d.dataset.botwork")).unwrap(),
        "Dataset |\"d\"| {\n    Row |\"r\"| Values |1|\n}\n"
    );
    let (status, _, _) = cli(
        directory.path(),
        &["--format", "--check", "--file", "x.botwork"],
    );
    assert_eq!(status, Some(2));
}

#[test]
fn the_documented_layout_is_canonical() {
    let guide =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/format.md")).unwrap();
    let fence = "## Layout\n\n```text\n";
    let start = guide.find(fence).unwrap() + fence.len();
    let example = &guide[start..start + guide[start..].find("```").unwrap()];
    assert_eq!(
        format("layout.botwork", example, SourceKind::Script).unwrap(),
        example
    );
}
