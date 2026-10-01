//! `docs/statements.md` is generated from the built-in statement catalogue, so
//! its signatures, kinds, and documented errors always match the binary. Every
//! statement must belong to exactly one topic page and be called by at least one
//! executed example. Regenerate the page with
//! `BOTWORK_UPDATE_STATEMENTS=1 cargo test --test statement_reference`.
#[path = "support/markdown.rs"]
mod markdown;

use botwork::core::{
    ast::{
        AccessSegment, AssignmentValue, Block, Call, ElseBranch, Expr, ExprKind, Program,
        Statement, StatementKind,
    },
    eval::Context,
    run::RunLimits,
    signature::StatementSignature,
    suite::Suite,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs,
    path::Path,
};

/// Topic pages in reference order: each documents and demonstrates its statements.
const TOPICS: [(&str, &str); 8] = [
    ("builtins.md", "Built-ins"),
    ("collections.md", "Collections"),
    ("strings.md", "Strings"),
    ("datetime.md", "Dates and times"),
    ("operating-system.md", "Files, environment, and paths"),
    ("processes.md", "Processes"),
    ("http.md", "HTTP"),
    ("structured-data.md", "JSON and CSV data"),
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn catalogue() -> Vec<StatementSignature> {
    let mut context = Context::with_limits(RunLimits::default()).unwrap();
    context.init_statements();
    let mut signatures: Vec<_> = context
        .statement_signatures()
        .into_iter()
        .cloned()
        .collect();
    signatures.sort_by_key(|signature| header(signature).to_lowercase());
    signatures
}

fn header(signature: &StatementSignature) -> &str {
    signature.header().text().trim()
}

fn calls_in_expression(expression: &Expr, calls: &mut BTreeSet<String>) {
    match &expression.kind {
        ExprKind::Call(call) => calls_in_call(call, calls),
        ExprKind::Access { base, segments } => {
            calls_in_expression(base, calls);
            for segment in segments {
                if let AccessSegment::Computed { index, .. } = segment {
                    calls_in_expression(index, calls);
                }
            }
        }
        ExprKind::Array(values) => values
            .iter()
            .for_each(|value| calls_in_expression(value, calls)),
        ExprKind::Map(entries) => entries
            .iter()
            .for_each(|(_, value)| calls_in_expression(value, calls)),
        ExprKind::Unary { operand, .. } => calls_in_expression(operand, calls),
        ExprKind::Binary { left, right, .. } => {
            calls_in_expression(left, calls);
            calls_in_expression(right, calls);
        }
        _ => {}
    }
}

fn calls_in_call(call: &Call, calls: &mut BTreeSet<String>) {
    calls.insert(call.signature.clone());
    call.arguments
        .iter()
        .for_each(|argument| calls_in_expression(argument, calls));
}

fn calls_in_block(block: &Block, calls: &mut BTreeSet<String>) {
    block
        .statements
        .iter()
        .for_each(|statement| calls_in_statement(statement, calls));
}

fn calls_in_statement(statement: &Statement, calls: &mut BTreeSet<String>) {
    match statement.kind() {
        StatementKind::Assign { value, .. } => match value {
            AssignmentValue::Expression(expression) => calls_in_expression(expression, calls),
            AssignmentValue::Call(call) => calls_in_call(call, calls),
        },
        StatementKind::Define(definition) => calls_in_block(&definition.body, calls),
        StatementKind::Invoke(call) => calls_in_call(call, calls),
        StatementKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            calls_in_expression(condition, calls);
            calls_in_block(then_branch, calls);
            match else_branch {
                Some(ElseBranch::Block(block)) => calls_in_block(block, calls),
                Some(ElseBranch::If(statement)) => calls_in_statement(statement, calls),
                None => {}
            }
        }
        StatementKind::For { iterable, body, .. } => {
            calls_in_expression(iterable, calls);
            calls_in_block(body, calls);
        }
        StatementKind::While { condition, body } => {
            calls_in_expression(condition, calls);
            calls_in_block(body, calls);
        }
        StatementKind::Poll { options, body, .. } => {
            calls_in_expression(options, calls);
            calls_in_block(body, calls);
        }
        StatementKind::Try { body, handler, .. } => {
            calls_in_block(body, calls);
            calls_in_block(handler, calls);
        }
        StatementKind::Finally { body, cleanup } => {
            calls_in_block(body, calls);
            calls_in_block(cleanup, calls);
        }
        StatementKind::Return(Some(expression)) => calls_in_expression(expression, calls),
        _ => {}
    }
}

fn program_calls(program: &Program) -> BTreeSet<String> {
    let mut calls = BTreeSet::new();
    program
        .statements
        .iter()
        .for_each(|statement| calls_in_statement(statement, &mut calls));
    calls
}

fn suite_calls(name: &str, source: &str) -> BTreeSet<String> {
    let suite = Suite::parse(name, source).unwrap_or_else(|error| panic!("{name}: {error}"));
    (0..suite.cases().len())
        .flat_map(|index| program_calls(&suite.program(index).unwrap()))
        .collect()
}

/// Executed examples in link order: documented examples, then example files.
fn examples() -> Vec<(String, BTreeSet<String>)> {
    let mut found = Vec::new();
    let mut documents = vec![root().join("README.md")];
    documents.extend(
        fs::read_dir(root().join("docs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md")),
    );
    documents.sort();
    for path in documents {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let text = fs::read_to_string(&path).unwrap();
        for block in markdown::blocks(&text).unwrap() {
            let link = if name == "README.md" {
                format!(
                    "[README](../README.md) (`{}`)",
                    block.id.as_deref().unwrap_or("")
                )
            } else {
                format!("[{name}]({name}) (`{}`)", block.id.as_deref().unwrap_or(""))
            };
            match block.language.as_str() {
                "botwork" => {
                    let program = Program::parse(&name, &block.source).unwrap();
                    found.push((link, program_calls(&program)));
                }
                "botwork-suite" => found.push((link, suite_calls(&name, &block.source))),
                _ => {}
            }
        }
    }
    let mut files: Vec<_> = fs::read_dir(root().join("examples"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "botwork")
        })
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let text = fs::read_to_string(&path).unwrap();
        let link = format!("[examples/{name}](../examples/{name})");
        let calls = if name.ends_with(".suite.botwork") {
            suite_calls(&name, &text)
        } else {
            program_calls(&Program::parse(&name, &text).unwrap())
        };
        found.push((link, calls));
    }
    found
}

/// The single topic page whose statement table lists a statement.
fn topic(signature: &StatementSignature) -> usize {
    let header = header(signature);
    let escaped = header.replace('|', "\\|");
    let pages: Vec<usize> = TOPICS
        .iter()
        .enumerate()
        .filter(|(_, (page, _))| {
            let text = fs::read_to_string(root().join("docs").join(page)).unwrap();
            // A topic page lists its statements as the first cell of a table row.
            text.lines().any(|line| {
                line.starts_with(&format!("| `{escaped}`"))
                    || line.starts_with(&format!("| `{header}`"))
            })
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        pages.len(),
        1,
        "`{header}` must be documented by exactly one topic page: {:?}",
        pages
            .iter()
            .map(|index| TOPICS[*index].0)
            .collect::<Vec<_>>()
    );
    pages[0]
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

fn render() -> String {
    let signatures = catalogue();
    let examples = examples();
    let mut sections: BTreeMap<usize, Vec<&StatementSignature>> = BTreeMap::new();
    for signature in &signatures {
        sections
            .entry(topic(signature))
            .or_default()
            .push(signature);
    }
    let mut page = String::new();
    let mut missing = Vec::new();
    writeln!(
        page,
        "# Statement reference

<!-- Generated by tests/statement_reference.rs from the statement catalogue.
     Regenerate with: BOTWORK_UPDATE_STATEMENTS=1 cargo test --test statement_reference -->

This page lists all {} built-in statements with their parameters, return kinds,
and documented errors, generated from the same catalogue as
`botwork --statement-help`. Each entry links to an executed example that calls
it; the topic pages explain the statements in depth.

- **Kinds.** `Any` accepts every value kind. A list such as `String | Array`
  accepts any of its kinds.
- **Errors.** Each code is listed with the condition that raises it; see
  [diagnostic codes](diagnostics.md). Any statement can also stop with BW5001 or
  BW5002 when its run is cancelled or times out.
- **Prerequisites.** When a statement documents an error for a missing
  prerequisite, such as asynchronous execution or Linux, its entry lists it
  under **Requires**; other requirements appear in the description. The CLI
  always runs asynchronously. No built-in statement needs an external adapter;
  adapters will list their prerequisites here as they arrive.
",
        signatures.len()
    )
    .unwrap();
    for (index, members) in &sections {
        let (_, title) = TOPICS[*index];
        writeln!(page, "- [{title}](#{}) ({})", anchor(title), members.len()).unwrap();
    }
    for (index, members) in &sections {
        let (file, title) = TOPICS[*index];
        writeln!(
            page,
            "\n## {title}\n\nSee [{file}]({file}) for details and examples."
        )
        .unwrap();
        for signature in members {
            writeln!(page, "\n### `{}`\n", header(signature)).unwrap();
            if !signature.documentation().is_empty() {
                writeln!(page, "{}\n", signature.documentation()).unwrap();
            }
            if !signature.parameters().is_empty() {
                writeln!(page, "| Parameter | Accepts |\n| --- | --- |").unwrap();
                for parameter in signature.parameters() {
                    writeln!(
                        page,
                        "| `{}` | {} |",
                        parameter.name,
                        cell(&parameter.accepted.to_string())
                    )
                    .unwrap();
                }
                writeln!(page).unwrap();
            }
            writeln!(page, "Returns: {}.", signature.return_kinds()).unwrap();
            let requires: Vec<String> = signature
                .documented_errors()
                .iter()
                .filter_map(|error| {
                    let need = error.description.strip_prefix("Requires ")?;
                    Some(format!("{} ({})", need.trim_end_matches('.'), error.code))
                })
                .collect();
            if !requires.is_empty() {
                writeln!(page, "\nRequires: {}.", requires.join("; ")).unwrap();
            }
            if !signature.documented_errors().is_empty() {
                writeln!(page, "\nErrors:\n").unwrap();
                for error in signature.documented_errors() {
                    writeln!(page, "- {}: {}", error.code, error.description).unwrap();
                }
            }
            let Some((example, _)) = examples
                .iter()
                .find(|(_, calls)| calls.contains(signature.normalized()))
            else {
                missing.push(header(signature).to_owned());
                continue;
            };
            writeln!(page, "\nExample: {example}.").unwrap();
        }
    }
    assert!(
        missing.is_empty(),
        "no executed example calls: {missing:#?}"
    );
    page
}

fn anchor(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter_map(|character| match character {
            ' ' => Some('-'),
            character if character.is_alphanumeric() || character == '-' => Some(character),
            _ => None,
        })
        .collect()
}

#[test]
fn statement_reference_matches_the_catalogue_and_executed_examples() {
    let path = root().join("docs/statements.md");
    let generated = render();
    if std::env::var_os("BOTWORK_UPDATE_STATEMENTS").is_some() {
        fs::write(&path, &generated).unwrap();
    }
    let current = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == generated,
        "docs/statements.md is out of date; regenerate it with \
         BOTWORK_UPDATE_STATEMENTS=1 cargo test --test statement_reference"
    );
}
