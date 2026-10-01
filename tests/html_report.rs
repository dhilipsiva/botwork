#[path = "support/cli_harness.rs"]
#[allow(dead_code)]
mod cli_harness;
use cli_harness::Harness;
use serde_json::Value;
use std::{fs, path::Path, process::Output, time::Duration};

const TAGS: [&str; 29] = [
    "html", "head", "meta", "title", "style", "body", "main", "h1", "h2", "h3", "dl", "dt", "dd",
    "table", "tr", "th", "td", "a", "span", "code", "mark", "pre", "details", "summary", "p", "br",
    "div", "ul", "li",
];

fn command(harness: &Harness, arguments: &[&str]) -> Output {
    harness
        .command("html", arguments, Duration::from_secs(60))
        .unwrap()
}

fn write(harness: &Harness, name: &str, source: &str) {
    fs::write(harness.workspace.join(name), source).unwrap();
}

/// Read the page and check what every page satisfies: the generator marker, a
/// policy that forbids scripts, only the renderer's own tags, and links that
/// stay within the page, beside the report, or on local files.
fn page(harness: &Harness, name: &str) -> String {
    let html = fs::read_to_string(harness.workspace.join(name)).unwrap();
    assert!(html.starts_with("<!doctype html>\n"));
    assert!(html.contains(r#"<meta name="generator" content="botwork-report-html 1">"#));
    assert!(html.contains(
        r#"<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">"#
    ));
    let mut rest = html.as_str();
    while let Some(index) = rest.find('<') {
        rest = &rest[index + 1..];
        let tag: String = rest
            .trim_start_matches('/')
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric())
            .collect();
        assert!(
            TAGS.contains(&tag.as_str()) || rest.starts_with("!doctype"),
            "unexpected tag <{tag}"
        );
    }
    for link in html.split("href=\"").skip(1) {
        let target = &link[..link.find('"').unwrap()];
        assert!(
            target.starts_with("#run-")
                || target.starts_with("file:///")
                || target
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-._~/%".contains(&byte)),
            "{target}"
        );
        assert!(!target.contains(".."), "{target}");
    }
    html
}

/// The run's collapsible section.
fn section(html: &str, number: usize) -> &str {
    let start = html
        .find(&format!(r#"id="run-{number}""#))
        .expect("run section");
    let end = html[start..].find("</details>").unwrap() + start;
    &html[start..end]
}

#[test]
fn html_reports_show_runs_timings_sources_logs_and_the_json_verdict() {
    let harness = Harness::new();
    write(&harness, "ok.botwork", "Sleep |20|\nLog |\"ready\"|");
    write(
        &harness,
        "fail.botwork",
        "|x| = |1|\n  Assert |x| Equals |2|\nLog |\"never\"|",
    );
    let output = command(
        &harness,
        &[
            "--file",
            "ok.botwork",
            "--file",
            "fail.botwork",
            "--report-html",
            "report.html",
            "--report-json",
            "report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let html = page(&harness, "report.html");
    let json: Value =
        serde_json::from_slice(&fs::read(harness.workspace.join("report.json")).unwrap()).unwrap();
    assert!(html.contains("<title>Botwork report: failed</title>"));
    assert!(html.contains("<dt>Exit code</dt><dd>1</dd>"));
    assert!(html.contains("<dt>Delivery</dt><dd>complete</dd>"));
    let counts = format!(
        "<tr><td>{}</td><td>{}</td><td>{}</td><td>0</td></tr>",
        json["verdict"]["cases"]["total"],
        json["verdict"]["cases"]["succeeded"],
        json["verdict"]["cases"]["failed"]
    );
    assert!(
        html.contains(&counts),
        "the summary agrees with the JSON verdict"
    );
    let passed = section(&html, 1);
    assert!(
        passed.starts_with(r#"id="run-1">"#),
        "passing runs start collapsed"
    );
    assert!(passed.contains("<mark>Sleep |20|</mark>"));
    assert!(passed.contains(r#"<div class="bar"><span style="left:"#));
    assert!(passed.contains("<pre>ready</pre>"));
    let failed = section(&html, 2);
    assert!(
        failed.starts_with(r#"id="run-2" open>"#),
        "failures start open"
    );
    assert!(failed.contains("<h3>Error BW9001 "));
    assert!(failed.contains("At <code>fail.botwork:2:3</code> "));
    assert!(failed.contains(r#"<code class="source">  <mark>Assert |x| Equals |2|</mark></code>"#));
    assert!(failed.contains("<code>BW9001</code>"));
    assert!(
        !failed.contains("never"),
        "statements after the failure never ran"
    );
    assert!(html.contains(r##"<a href="#run-2">2</a>"##));
}

#[test]
fn recorded_content_is_rendered_as_text() {
    let harness = Harness::new();
    write(
        &harness,
        "hostile.suite.botwork",
        r#"Suite |"hostile"| Named |"<b>suite</b>"| {
    Case |"quote"| Named |"\"><img src=x onerror=alert(1)>"| {
        Log |"<script>alert('log')</script>"|
        Fail |"</pre><iframe src=//example.com>"|
    }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "hostile.suite.botwork",
            "--report-html",
            "report.html",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let html = page(&harness, "report.html");
    assert!(html.contains("&lt;script&gt;alert(&#39;log&#39;)&lt;/script&gt;"));
    assert!(html.contains("&quot;&gt;&lt;img src=x onerror=alert(1)&gt;"));
    assert!(html.contains("&lt;/pre&gt;&lt;iframe src=//example.com&gt;"));
    for raw in ["<script", "<img", "<iframe", "<b>"] {
        assert!(!html.contains(raw), "{raw} must be text");
    }
}

#[test]
fn artifact_links_stay_attached_to_their_runs() {
    let harness = Harness::new();
    write(&harness, "a.botwork", "Assert |1| Equals |2|");
    write(&harness, "b.botwork", "No Operation");
    write(&harness, "c.botwork", "Assert |\"x\"| Equals |\"y\"|");
    fs::create_dir(harness.workspace.join("out")).unwrap();
    let output = command(
        &harness,
        &[
            "--file",
            "a.botwork",
            "--file",
            "b.botwork",
            "--file",
            "c.botwork",
            "--jobs",
            "3",
            "--assertion-artifacts",
            "out/evidence",
            "--report-html",
            "out/report.html",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let html = page(&harness, "out/report.html");
    assert!(!section(&html, 2).contains("href="));
    for (number, file) in [(1, "a.botwork"), (3, "c.botwork")] {
        let run = section(&html, number);
        let links: Vec<_> = run
            .split("href=\"")
            .skip(1)
            .map(|link| &link[..link.find('"').unwrap()])
            .collect();
        assert_eq!(links.len(), 1, "{run}");
        assert!(links[0].starts_with("evidence/"), "relative to the report");
        let evidence: Value = serde_json::from_slice(
            &fs::read(harness.workspace.join("out").join(links[0])).unwrap(),
        )
        .unwrap();
        assert_eq!(evidence["identity"]["run"], number);
        assert_eq!(evidence["identity"]["file"], file);
    }
}

#[test]
fn suite_html_reports_show_rows_skips_and_shared_fixtures() {
    let harness = Harness::new();
    write(
        &harness,
        "rows.suite.botwork",
        r#"Suite |"rows"| {
    Dataset |"numbers"| {
        Row |"one"| Values |1|
    }
    SuiteSetup { No Operation }
    SuiteTeardown { No Operation }
    Case |"small"| Using |"numbers"| As |number| { Assert |number < 2| }
}"#,
    );
    write(
        &harness,
        "broken.suite.botwork",
        r#"Suite |"broken"| {
    SuiteSetup { |x| = |missing| }
    SuiteTeardown { No Operation }
    Case |"blocked"| { No Operation }
}"#,
    );
    let output = command(
        &harness,
        &[
            "--suite",
            "rows.suite.botwork",
            "--suite",
            "broken.suite.botwork",
            "--jobs",
            "1",
            "--report-html",
            "report.html",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let html = page(&harness, "report.html");
    assert!(html.contains("<h2>Shared fixtures</h2>"));
    assert!(html.contains(
        r#"<details class="run" open><summary>broken <span class="status failed">failed</span></summary><h3>Error BW2001 "#
    ));
    assert!(html.contains("At <code>broken.suite.botwork:2:25</code> "));
    assert!(html.contains(r#"<td>numbers / one</td>"#));
    let skipped = section(&html, 2);
    assert!(skipped.contains("<dt>Skipped</dt><dd>suite setup did not complete</dd>"));
    assert!(skipped.contains(r#"<span class="status skipped">skipped</span>"#));
}

#[test]
fn html_reports_are_incomplete_until_the_verdict() {
    let harness = Harness::new();
    write(
        &harness,
        "peek.botwork",
        "Log |@{ Read File |\"report.html\"| }|",
    );
    let output = command(
        &harness,
        &["--file", "peek.botwork", "--report-html", "report.html"],
    );
    assert_eq!(output.status.code(), Some(0));
    let marker = String::from_utf8(output.stdout).unwrap();
    assert!(marker.contains("<title>Botwork report: incomplete</title>"));
    assert!(marker.contains(r#"<p class="banner">This report is incomplete"#));
    assert!(page(&harness, "report.html").contains("<title>Botwork report: succeeded</title>"));

    let output = command(
        &harness,
        &[
            "--suite",
            "absent.suite.botwork",
            "--report-html",
            "report.html",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(page(&harness, "report.html").contains("<title>Botwork report: incomplete</title>"));
}

#[test]
fn changed_sources_are_not_excerpted() {
    let harness = Harness::new();
    write(
        &harness,
        "self.botwork",
        "No Operation\nWrite File |\"self.botwork\"| Text |\"\\n\\nreplaced\"|",
    );
    let output = command(
        &harness,
        &["--file", "self.botwork", "--report-html", "report.html"],
    );
    assert_eq!(output.status.code(), Some(0));
    let html = page(&harness, "report.html");
    assert!(
        !html.contains("<mark>"),
        "no excerpt may come from the changed file"
    );
    assert_eq!(html.matches("source unavailable").count(), 2);
}

#[test]
fn unrelated_outputs_are_never_replaced() {
    let harness = Harness::new();
    write(&harness, "effect.botwork", "Log |\"ran\"|");
    write(&harness, "index.html", "<!doctype html><p>keep me</p>");
    let output = command(
        &harness,
        &["--file", "effect.botwork", "--report-html", "index.html"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "no run starts");
    assert!(String::from_utf8_lossy(&output.stderr).contains("never replaced"));
    assert_eq!(
        fs::read_to_string(harness.workspace.join("index.html")).unwrap(),
        "<!doctype html><p>keep me</p>"
    );
    // A report of another version is still a report; a marker without one is not.
    for (marker, replaced) in [
        (
            r#"<meta name="generator" content="botwork-report-html 2">"#,
            true,
        ),
        (
            r#"<meta name="generator" content="botwork-report-html ">"#,
            false,
        ),
        (
            r#"<meta name="generator" content="botwork-report-html 2x">"#,
            false,
        ),
    ] {
        write(&harness, "other.html", &format!("<!doctype html>{marker}"));
        let output = command(
            &harness,
            &["--file", "effect.botwork", "--report-html", "other.html"],
        );
        assert_eq!(
            output.status.code(),
            Some(if replaced { 0 } else { 1 }),
            "{marker}"
        );
        let page = fs::read_to_string(harness.workspace.join("other.html")).unwrap();
        assert_eq!(page.contains("botwork-report-html 1"), replaced, "{marker}");
    }
    let output = command(
        &harness,
        &[
            "--file",
            "effect.botwork",
            "--report-html",
            "same",
            "--report-json",
            "same",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "one path cannot hold both reports"
    );
    assert!(output.stdout.is_empty());
    fs::create_dir(harness.workspace.join("folder")).unwrap();
    let output = command(
        &harness,
        &["--file", "effect.botwork", "--report-html", "folder"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(Path::new(&harness.workspace.join("folder")).is_dir());
    // A previous report is replaced.
    for _ in 0..2 {
        let output = command(
            &harness,
            &["--file", "effect.botwork", "--report-html", "report.html"],
        );
        assert_eq!(output.status.code(), Some(0));
    }
}

/// The contents of the first fenced block after `opening`.
fn fenced<'a>(document: &'a str, opening: &str) -> &'a str {
    let start = document.find(opening).expect("documented block") + opening.len();
    let length = document[start..].find("\n```\n").expect("closing fence") + 1;
    &document[start..start + length]
}

#[test]
fn the_documented_example_shows_what_the_page_describes() {
    let document = include_str!("../docs/html-report.md");
    let script = fenced(
        document,
        "<!-- botwork-test: html-report-example -->\n```botwork\n",
    );
    let harness = Harness::new();
    write(&harness, "checkout.botwork", script);
    let output = command(
        &harness,
        &["--file", "checkout.botwork", "--report-html", "report.html"],
    );
    assert_eq!(output.status.code(), Some(0));
    let html = page(&harness, "report.html");
    let run = section(&html, 1);
    assert_eq!(
        run.matches("<tr><td>").count(),
        6 + 2,
        "six statements, two logs"
    );
    for expected in [
        "<mark>|items| = |[3, 4]|</mark>",
        "<mark>For |item| In |items| {</mark>",
        "<pre>7</pre>",
        r#"<div class="bar">"#,
    ] {
        assert!(run.contains(expected), "{expected}\n{run}");
    }
}
