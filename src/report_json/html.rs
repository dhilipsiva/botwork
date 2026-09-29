//! The HTML report: one self-contained page per invocation, rendered from the
//! same document as the JSON report. Every recorded value is escaped as text, and
//! a Content-Security-Policy forbids scripts and remote content.
use super::{Document, Run};
use crate::{BWErr, CliError, Diagnostic};
use botwork::core::{
    acceptance::CaseStatus,
    report::{ArtifactRecord, ErrorRecord, LogRecord, RunRecord, SourceLocation, StatementRecord},
};
use std::{
    collections::HashMap,
    fmt::{self, Write},
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

/// Identifies a replaceable HTML report within its first bytes.
pub(super) const GENERATOR: &str = r#"<meta name="generator" content="botwork-report-html 1">"#;
const RECOGNITION_BYTES: u64 = 4096;
const MAX_SOURCE_FILES: usize = 64;
const MAX_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
/// Columns are counted in Unicode scalars; longer line prefixes get no excerpt.
const MAX_LINE_PREFIX_BYTES: usize = 64 * 1024;
const EXCERPT_BEFORE: usize = 60;
const EXCERPT_MARKED: usize = 120;
const EXCERPT_AFTER: usize = 40;

const STATUSES: [CaseStatus; 9] = [
    CaseStatus::Succeeded,
    CaseStatus::ExpectedFailure,
    CaseStatus::Failed,
    CaseStatus::UnexpectedPass,
    CaseStatus::Skipped,
    CaseStatus::Cancelled,
    CaseStatus::TimedOut,
    CaseStatus::LimitExceeded,
    CaseStatus::Interrupted,
];

const STYLE: &str = "
body{font:14px/1.45 system-ui,sans-serif;margin:0;color:#1d2330;background:#f6f7f9}
main{max-width:1100px;margin:0 auto;padding:24px}
h1{font-size:22px;margin:0 0 8px}h2{font-size:17px;margin:28px 0 8px}h3{font-size:14px;margin:14px 0 6px}
table{border-collapse:collapse;width:100%;background:#fff}
th,td{border-bottom:1px solid #e3e6eb;padding:5px 8px;text-align:left;vertical-align:top}
th{font-weight:600;background:#eef0f3}
code,pre{font:12.5px/1.4 ui-monospace,monospace}
pre{margin:0;white-space:pre-wrap;overflow-wrap:anywhere}
dl.facts{display:grid;grid-template-columns:max-content 1fr;gap:2px 16px;margin:8px 0}
dl.facts dt{color:#5b6475}dl.facts dd{margin:0}
.status{display:inline-block;padding:1px 8px;border-radius:10px;font-size:12px;font-weight:600;color:#fff;background:#6b7280}
.succeeded,.expected_failure{background:#1f883d}
.failed,.unexpected_pass{background:#cf222e}
.cancelled,.timed_out,.limit_exceeded{background:#bc4c00}
.interrupted,.incomplete{background:#8250df}
.skipped{background:#6b7280}
.banner{padding:10px 14px;border-radius:6px;background:#fff8c5;border:1px solid #d4a72c;margin:12px 0}
details.run{background:#fff;border:1px solid #e3e6eb;border-radius:6px;margin:10px 0;padding:0 12px}
details.run>summary{cursor:pointer;padding:10px 0;font-weight:600}
.bar{position:relative;height:10px;min-width:120px;background:#eef0f3;border-radius:3px}
.bar>span{position:absolute;top:0;height:10px;min-width:2px;border-radius:3px;background:#0969da}
.source mark{background:#fff1a8}
.muted{color:#5b6475}
";

/// Text escaped for HTML content and double- or single-quoted attributes.
struct Text<'a>(&'a str);

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut rest = self.0;
        while let Some(index) = rest.find(['&', '<', '>', '"', '\'', '\0']) {
            f.write_str(&rest[..index])?;
            f.write_str(match rest.as_bytes()[index] {
                b'&' => "&amp;",
                b'<' => "&lt;",
                b'>' => "&gt;",
                b'"' => "&quot;",
                b'\'' => "&#39;",
                _ => "\u{fffd}",
            })?;
            rest = &rest[index + 1..];
        }
        f.write_str(rest)
    }
}

/// Accept an existing file only when it starts like a Botwork HTML report.
pub(super) fn recognized(path: &Path) -> Result<(), CliError> {
    let mut start = Vec::new();
    File::open(path)
        .and_then(|file| file.take(RECOGNITION_BYTES).read_to_end(&mut start))
        .map_err(|error| rejected(path, &error.to_string()))?;
    if String::from_utf8_lossy(&start).contains(GENERATOR) {
        Ok(())
    } else {
        Err(rejected(
            path,
            "an existing file that is not an HTML report is never replaced",
        ))
    }
}

fn rejected(path: &Path, reason: &str) -> CliError {
    Diagnostic::new(BWErr::OutputError(format!(
        "HTML report {}: {reason}",
        path.display()
    )))
    .into()
}

pub(super) fn duration(us: u64) -> String {
    match us {
        0..=999 => format!("{us} µs"),
        1_000..=999_999 => format!("{:.1} ms", us as f64 / 1e3),
        _ => format!("{:.2} s", us as f64 / 1e6),
    }
}

fn status(status: CaseStatus) -> String {
    format!(
        r#"<span class="status {}">{}</span>"#,
        status.as_str(),
        status.label()
    )
}

fn location(location: &SourceLocation) -> String {
    format!(
        "{}:{}:{}",
        Text(&location.file),
        location.line,
        location.column
    )
}

/// Percent-encode a path's bytes, keeping unreserved characters and `/`.
fn encode(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()).to_vec();
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().replace('\\', "/").into_bytes();
    let mut encoded = String::new();
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Resolve `.` and `..` without touching the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// A link to an artifact: relative when it lies beneath the report's
/// directory, so both can be archived together, and a file URL otherwise.
pub(super) fn href(path: &str, directory: &Path) -> String {
    let path = Path::new(path);
    let absolute = std::env::current_dir()
        .map(|current| current.join(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let absolute = fs::canonicalize(&absolute).unwrap_or_else(|_| lexical(&absolute));
    match absolute.strip_prefix(directory) {
        Ok(relative)
            if relative
                .components()
                .all(|component| matches!(component, Component::Normal(_))) =>
        {
            encode(relative)
        }
        _ => format!("file://{}", encode(&absolute)),
    }
}

/// Source files read once, bounded, for statement and error excerpts.
#[derive(Default)]
pub(super) struct Sources {
    files: HashMap<String, Option<(String, Vec<usize>)>>,
}

pub(super) struct Excerpt {
    before: String,
    marked: String,
    after: String,
}

fn tail(text: &str, maximum: usize) -> String {
    let count = text.chars().count();
    if count <= maximum {
        return text.to_owned();
    }
    format!(
        "…{}",
        text.chars().skip(count - maximum).collect::<String>()
    )
}

fn head(text: &str, maximum: usize) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(maximum).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

impl Sources {
    fn file(&mut self, name: &str) -> Option<&(String, Vec<usize>)> {
        if name.starts_with('<') {
            return None;
        }
        if !self.files.contains_key(name) {
            if self.files.len() >= MAX_SOURCE_FILES {
                return None;
            }
            let text = File::open(name)
                .ok()
                .filter(|file| file.metadata().is_ok_and(|metadata| metadata.is_file()))
                .and_then(|file| {
                    let mut text = String::new();
                    file.take(MAX_SOURCE_BYTES + 1)
                        .read_to_string(&mut text)
                        .ok()
                        .filter(|_| text.len() as u64 <= MAX_SOURCE_BYTES)
                        .map(|_| text)
                })
                .map(|text| {
                    let starts = std::iter::once(0)
                        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
                        .collect();
                    (text, starts)
                });
            self.files.insert(name.to_owned(), text);
        }
        self.files.get(name)?.as_ref()
    }

    /// The recorded location's first line, when the file still places the
    /// location at the recorded line and column.
    pub(super) fn excerpt(&mut self, location: &SourceLocation) -> Option<Excerpt> {
        let (text, starts) = self.file(&location.file)?;
        let (start, end) = (location.start_byte, location.end_byte);
        if start > end
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            return None;
        }
        let line = starts.partition_point(|&line_start| line_start <= start);
        let line_start = starts[line - 1];
        if start - line_start > MAX_LINE_PREFIX_BYTES {
            return None;
        }
        let column = text[line_start..start].chars().count() + 1;
        if (line, column) != (location.line, location.column) {
            return None;
        }
        let line_end = starts
            .get(line)
            .map_or(text.len(), |next| next - 1)
            .max(start);
        let marked_end = end.min(line_end);
        Some(Excerpt {
            before: tail(&text[line_start..start], EXCERPT_BEFORE),
            marked: head(&text[start..marked_end], EXCERPT_MARKED),
            after: head(
                text[marked_end..line_end].trim_end_matches('\r'),
                EXCERPT_AFTER,
            ),
        })
    }

    fn render(&mut self, location: &SourceLocation, out: &mut String) {
        match self.excerpt(location) {
            Some(excerpt) => {
                let _ = write!(
                    out,
                    r#"<code class="source">{}<mark>{}</mark>{}</code>"#,
                    Text(&excerpt.before),
                    Text(excerpt.marked.trim_end_matches('\r')),
                    Text(&excerpt.after)
                );
            }
            None => out.push_str(r#"<span class="muted">source unavailable</span>"#),
        }
    }
}

fn fact(out: &mut String, name: &str, value: impl fmt::Display) {
    let _ = write!(out, "<dt>{name}</dt><dd>{value}</dd>");
}

fn error(out: &mut String, sources: &mut Sources, error: &ErrorRecord) {
    let _ = write!(
        out,
        r#"<h3>Error {} {}</h3><pre>{}</pre>"#,
        Text(error.code),
        status(error.status),
        Text(&error.message)
    );
    if error.truncated {
        out.push_str(r#"<p class="muted">Message truncated.</p>"#);
    }
    if let Some(at) = &error.location {
        let _ = write!(out, "<p>At <code>{}</code> ", location(at));
        sources.render(at, out);
        out.push_str("</p>");
    }
    if !error.causes.is_empty() || error.omitted_causes != 0 {
        let causes: Vec<_> = error
            .causes
            .iter()
            .map(|code| Text(code).to_string())
            .collect();
        let _ = write!(out, "<p>Causes: <code>{}</code>", causes.join(", "));
        if error.omitted_causes != 0 {
            let _ = write!(out, " and {} more", error.omitted_causes);
        }
        out.push_str("</p>");
    }
}

fn statements(out: &mut String, sources: &mut Sources, record: &RunRecord) {
    if record.statements.is_empty() {
        return;
    }
    let total = record.duration_us.unwrap_or(0).max(1) as f64;
    out.push_str(
        "<h3>Statements</h3><table><tr><th>#</th><th>Kind</th><th>Source</th>\
         <th>Start</th><th>Duration</th><th>Timeline</th><th>Status</th></tr>",
    );
    for statement in &record.statements {
        let StatementRecord {
            index,
            kind,
            location: at,
            offset_us,
            duration_us,
            status: outcome,
            code,
        } = statement;
        let _ = write!(
            out,
            "<tr><td>{index}</td><td>{}</td><td><code>{}</code><br>",
            Text(kind),
            location(at)
        );
        sources.render(at, out);
        let _ = write!(
            out,
            "</td><td>{}</td><td>{}</td><td>",
            duration(*offset_us),
            duration_us.map_or_else(|| "—".to_owned(), duration)
        );
        if record.duration_us.is_some() {
            let left = (*offset_us as f64 / total * 100.0).min(100.0);
            let width = (duration_us.unwrap_or(0) as f64 / total * 100.0).min(100.0 - left);
            let _ = write!(
                out,
                r#"<div class="bar"><span style="left:{left:.1}%;width:{width:.1}%"></span></div>"#
            );
        }
        out.push_str("</td><td>");
        if let Some(outcome) = outcome {
            out.push_str(&status(*outcome));
        }
        if let Some(code) = code {
            let _ = write!(out, " <code>{}</code>", Text(code));
        }
        out.push_str("</td></tr>");
    }
    out.push_str("</table>");
    if record.omitted_statements != 0 {
        let _ = write!(
            out,
            r#"<p class="muted">{} more statements were not retained.</p>"#,
            record.omitted_statements
        );
    }
}

fn logs(out: &mut String, record: &RunRecord) {
    if record.logs.is_empty() && record.omitted_logs == 0 {
        return;
    }
    let _ = write!(
        out,
        "<h3>Logs</h3><p class=\"muted\">{} bytes logged in all.</p>\
         <table><tr><th>Statement</th><th>At</th><th>Bytes</th><th>Text</th></tr>",
        record.logged_bytes
    );
    for LogRecord {
        statement,
        offset_us,
        bytes,
        text,
        truncated,
    } in &record.logs
    {
        let _ = write!(
            out,
            "<tr><td>{}</td><td>{}</td><td>{bytes}</td><td><pre>{}</pre>{}</td></tr>",
            statement.map_or_else(|| "—".to_owned(), |index| index.to_string()),
            duration(*offset_us),
            Text(text),
            if *truncated {
                r#"<span class="muted">truncated</span>"#
            } else {
                ""
            }
        );
    }
    out.push_str("</table>");
    if record.omitted_logs != 0 {
        let _ = write!(
            out,
            r#"<p class="muted">{} more logs were not retained; stdout has the full output.</p>"#,
            record.omitted_logs
        );
    }
}

fn artifacts(out: &mut String, record: &RunRecord, directory: &Path) {
    if record.artifacts.is_empty() && record.omitted_artifacts == 0 {
        return;
    }
    out.push_str("<h3>Artifacts</h3><ul>");
    for ArtifactRecord { kind, path } in &record.artifacts {
        let _ = write!(
            out,
            r#"<li>{}: <a href="{}">{}</a></li>"#,
            Text(kind),
            Text(&href(path, directory)),
            Text(path)
        );
    }
    out.push_str("</ul>");
    if record.omitted_artifacts != 0 {
        let _ = write!(
            out,
            r#"<p class="muted">{} more artifacts were not retained.</p>"#,
            record.omitted_artifacts
        );
    }
}

fn identity_name(id: &str, name: &str) -> String {
    if id == name {
        Text(id).to_string()
    } else {
        format!("{} <span class=\"muted\">{}</span>", Text(name), Text(id))
    }
}

/// Render the complete page for `document`; artifact links are relative to
/// `directory`, the report's canonical directory.
pub(super) fn render(document: &Document<'_>, directory: &Path) -> String {
    let mut sources = Sources::default();
    let mut out = String::new();
    let overall = match document.verdict {
        Some(verdict) => status(verdict.status()),
        None => r#"<span class="status incomplete">incomplete</span>"#.to_owned(),
    };
    let _ = write!(
        out,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">{GENERATOR}\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Botwork report: {}</title><style>{STYLE}</style></head><body><main>\
         <h1>Botwork report {overall}</h1><dl class=\"facts\">",
        document
            .verdict
            .map_or("incomplete", |verdict| verdict.status().label())
    );
    fact(&mut out, "Mode", Text(document.mode));
    fact(&mut out, "Started", Text(document.started_at));
    fact(
        &mut out,
        "Finished",
        Text(document.finished_at.as_deref().unwrap_or("—")),
    );
    fact(
        &mut out,
        "Duration",
        document
            .duration_us
            .map_or_else(|| "—".to_owned(), duration),
    );
    fact(
        &mut out,
        "Exit code",
        document
            .exit_code
            .map_or_else(|| "—".to_owned(), |code| code.to_string()),
    );
    if let Some(verdict) = document.verdict {
        fact(&mut out, "Delivery", verdict.delivery().as_str());
    }
    out.push_str("</dl>");
    let Some(verdict) = document.verdict else {
        out.push_str(
            "<p class=\"banner\">This report is incomplete: the invocation has not finished, \
             or it stopped before every selected run had a record.</p></main></body></html>\n",
        );
        return out;
    };
    if verdict.delivery() == botwork::core::acceptance::Delivery::Interrupted {
        out.push_str(
            "<p class=\"banner\">This invocation was interrupted. Runs still running were \
             cancelled, or are shown as interrupted when a forced termination was reconciled.</p>",
        );
    }
    if !document.complete {
        out.push_str(
            "<p class=\"banner\">Not every selected run has a record: runs that never \
             started are not shown.</p>",
        );
    }
    out.push_str("<h2>Summary</h2><table><tr>");
    let counted: Vec<_> = STATUSES
        .iter()
        .filter(|outcome| verdict.cases().count(**outcome) != 0)
        .collect();
    out.push_str("<th>Runs</th>");
    for outcome in &counted {
        let _ = write!(out, "<th>{}</th>", status(**outcome));
    }
    let _ = write!(
        out,
        "<th>Fixture failures</th></tr><tr><td>{}</td>",
        verdict.cases().total()
    );
    for outcome in &counted {
        let _ = write!(out, "<td>{}</td>", verdict.cases().count(**outcome));
    }
    let _ = write!(out, "<td>{}</td></tr></table>", verdict.fixture_failures());
    if !document.fixtures.is_empty() {
        out.push_str("<h2>Shared fixtures</h2>");
        for fixture in document.fixtures {
            let _ = write!(
                out,
                r#"<details class="run"{}><summary>{} {}</summary>"#,
                if fixture.error.is_some() { " open" } else { "" },
                Text(&fixture.suite),
                status(fixture.status)
            );
            if let Some(failure) = &fixture.error {
                error(&mut out, &mut sources, failure);
            }
            out.push_str("</details>");
        }
    }
    out.push_str(
        "<h2>Runs</h2><table><tr><th>#</th><th>Run</th><th>Dataset row</th>\
         <th>Status</th><th>Duration</th><th>Error</th></tr>",
    );
    for run in &document.runs {
        let (number, identity, outcome, failure, took) = match run {
            Run::Full(entry) => (
                entry.number,
                &entry.record.identity,
                entry.record.status,
                entry.record.error.as_ref().map(|error| error.code),
                entry.record.duration_us,
            ),
            Run::Summary(summary) => (
                summary.number,
                &summary.identity,
                summary.status,
                summary.error.as_deref(),
                None,
            ),
        };
        let _ = write!(
            out,
            r##"<tr><td><a href="#run-{number}">{number}</a></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"##,
            identity_name(&identity.id, &identity.name),
            match (&identity.dataset, &identity.row) {
                (Some(dataset), Some(row)) => format!("{} / {}", Text(dataset), Text(row)),
                _ => "—".to_owned(),
            },
            status(outcome),
            took.map_or_else(|| "—".to_owned(), duration),
            failure.map_or_else(String::new, |code| format!("<code>{}</code>", Text(code)))
        );
    }
    out.push_str("</table>");
    for run in &document.runs {
        match run {
            Run::Full(entry) => {
                let record = entry.record;
                let _ = write!(
                    out,
                    r#"<details class="run" id="run-{}"{}><summary>#{} {} {}</summary><dl class="facts">"#,
                    entry.number,
                    if record.status.failed() { " open" } else { "" },
                    entry.number,
                    identity_name(&record.identity.id, &record.identity.name),
                    status(record.status)
                );
                if let (Some(dataset), Some(row)) = (&record.identity.dataset, &record.identity.row)
                {
                    fact(&mut out, "Dataset", Text(dataset));
                    fact(&mut out, "Row", Text(row));
                }
                if let Some(reason) = record.skip_reason {
                    fact(
                        &mut out,
                        "Skipped",
                        match reason {
                            botwork::core::acceptance::SkipReason::SuiteSetupFailed => {
                                "suite setup did not complete"
                            }
                            botwork::core::acceptance::SkipReason::SuiteStopped => {
                                "suite control stopped"
                            }
                        },
                    );
                }
                if let Some(started) = &record.started_at {
                    fact(&mut out, "Started", Text(started));
                }
                if let Some(finished) = &record.finished_at {
                    fact(&mut out, "Finished", Text(finished));
                }
                if let Some(took) = record.duration_us {
                    fact(&mut out, "Duration", duration(took));
                }
                out.push_str("</dl>");
                if let Some(failure) = &record.error {
                    error(&mut out, &mut sources, failure);
                }
                statements(&mut out, &mut sources, record);
                logs(&mut out, record);
                artifacts(&mut out, record, directory);
                out.push_str("</details>");
            }
            Run::Summary(summary) => {
                let _ = write!(
                    out,
                    r#"<details class="run" id="run-{}"><summary>#{} {} {}</summary><p class="muted">Details omitted: the report's run budget was spent.</p></details>"#,
                    summary.number,
                    summary.number,
                    identity_name(&summary.identity.id, &summary.identity.name),
                    status(summary.status)
                );
            }
        }
    }
    out.push_str("</main></body></html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_markup_quotes_and_nul() {
        assert_eq!(
            Text("<a href='x'>\"&\0</a>").to_string(),
            "&lt;a href=&#39;x&#39;&gt;&quot;&amp;\u{fffd}&lt;/a&gt;"
        );
    }

    #[test]
    fn links_are_relative_beneath_the_report_and_encoded() {
        let directory = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(directory.path()).unwrap();
        let nested = base.join("evidence dir").join("a#1?.json");
        fs::create_dir_all(nested.parent().unwrap()).unwrap();
        fs::write(&nested, "{}").unwrap();
        assert_eq!(
            href(nested.to_str().unwrap(), &base),
            "evidence%20dir/a%231%3F.json"
        );
        let outside = base.join("..").join("elsewhere.json");
        let link = href(outside.to_str().unwrap(), &base);
        assert!(
            link.starts_with("file:///") && !link.contains(".."),
            "{link}"
        );
    }

    #[test]
    fn excerpts_require_the_recorded_line_and_column() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("source.botwork");
        fs::write(&file, "No Operation\r\n  Assert |ünï| Equals |2|\r\n").unwrap();
        let name = file.to_str().unwrap().to_owned();
        let start = "No Operation\r\n  ".len();
        let location = SourceLocation {
            file: name.clone(),
            start_byte: start,
            end_byte: start + "Assert |ünï| Equals |2|".len(),
            line: 2,
            column: 3,
            end_line: 2,
            end_column: 26,
        };
        let excerpt = Sources::default().excerpt(&location).unwrap();
        assert_eq!(
            (
                excerpt.before.as_str(),
                excerpt.marked.as_str(),
                excerpt.after.as_str()
            ),
            ("  ", "Assert |ünï| Equals |2|", "")
        );
        for moved in [
            SourceLocation {
                column: 4,
                ..location.clone()
            },
            SourceLocation {
                line: 1,
                ..location.clone()
            },
            SourceLocation {
                end_byte: 10_000,
                ..location.clone()
            },
            SourceLocation {
                file: "<builtin>".into(),
                ..location.clone()
            },
        ] {
            assert!(Sources::default().excerpt(&moved).is_none());
        }
        let long = SourceLocation {
            start_byte: 0,
            end_byte: 12,
            line: 1,
            column: 1,
            ..location
        };
        fs::write(&file, format!("{}\n", "x".repeat(300))).unwrap();
        let excerpt = Sources::default().excerpt(&long).unwrap();
        assert_eq!(excerpt.marked, "x".repeat(12));
        assert_eq!(excerpt.after, format!("{}…", "x".repeat(EXCERPT_AFTER)));
    }

    #[test]
    fn source_reads_are_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let at = |file: &Path, start: usize, column: usize| SourceLocation {
            file: file.to_str().unwrap().to_owned(),
            start_byte: start,
            end_byte: start + 1,
            line: 1,
            column,
            end_line: 1,
            end_column: column + 1,
        };
        let mut sources = Sources::default();
        for index in 0..=MAX_SOURCE_FILES {
            let file = directory.path().join(format!("{index}.botwork"));
            fs::write(&file, "x").unwrap();
            let excerpt = sources.excerpt(&at(&file, 0, 1));
            assert_eq!(excerpt.is_some(), index < MAX_SOURCE_FILES, "file {index}");
        }
        let large = directory.path().join("large.botwork");
        fs::write(&large, "x".repeat(MAX_SOURCE_BYTES as usize + 1)).unwrap();
        assert!(Sources::default().excerpt(&at(&large, 0, 1)).is_none());
        let wide = directory.path().join("wide.botwork");
        fs::write(&wide, "x".repeat(MAX_LINE_PREFIX_BYTES + 2)).unwrap();
        let mut sources = Sources::default();
        let edge = MAX_LINE_PREFIX_BYTES;
        assert!(sources.excerpt(&at(&wide, edge, edge + 1)).is_some());
        assert!(sources.excerpt(&at(&wide, edge + 1, edge + 2)).is_none());
    }

    #[test]
    fn durations_use_readable_units() {
        assert_eq!(duration(999), "999 µs");
        assert_eq!(duration(1_500), "1.5 ms");
        assert_eq!(duration(2_345_000), "2.35 s");
    }
}
