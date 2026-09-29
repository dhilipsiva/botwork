//! `--lsp` serves the Language Server Protocol on stdin and stdout, with
//! diagnostics, completion, hover, definitions, and references from the shared
//! language analysis.
#[path = "support/lsp_client.rs"]
mod lsp_client;

use lsp_client::{file_uri, range, Client};
use serde_json::{json, Value};
use std::process::{Command, Stdio};

#[test]
fn diagnostics_follow_edits_and_clear_on_close() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::start(directory.path());
    let uri = file_uri(&directory.path().join("main.botwork"));
    let diagnostics = client.open(&uri, "Log |1| |2|\nLog |missing|\n");
    assert_eq!(
        diagnostics,
        [
            json!({
                "range": range((0, 0), (0, 11)),
                "severity": 1,
                "code": "BW2002",
                "source": "botwork",
                "message": "Statement not defined: Log |1| |2| (undefined-statement)\nhelp: Did you mean `Log |value|`? Calls must match a definition's words and parameter positions.",
            }),
            json!({
                "range": range((1, 5), (1, 12)),
                "severity": 2,
                "code": "BW2001",
                "source": "botwork",
                "message": "`missing` is never assigned in a scope that reaches this read (undefined-variable)\nhelp: Assign it first, or supply it as an input variable with --var or --vars-file.",
            }),
        ]
    );
    // An incomplete edit reports the syntax error where `--check` does: at the
    // end of the text, as `2:1`.
    let broken = client.change(&uri, 2, "Log |1\n");
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0]["code"], "BW1001");
    assert_eq!(broken[0]["severity"], 1);
    assert_eq!(broken[0]["range"], range((1, 0), (1, 0)));
    assert!(broken[0]["message"]
        .as_str()
        .unwrap()
        .ends_with("\nhelp: Check the indicated token and close every pipe, bracket, brace, quote, and block comment."));
    // Fixing the error removes it.
    assert_eq!(client.change(&uri, 3, "Log |1|\n"), Vec::<Value>::new());
    client.change(&uri, 4, "Log |missing|\n");
    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": uri}}),
    );
    assert_eq!(client.diagnostics(&uri), Vec::<Value>::new());
    assert_eq!(client.finish(true), Some(0));
}

#[test]
fn navigation_counts_positions_in_utf16_code_units() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::start(directory.path());
    let uri = file_uri(&directory.path().join("main.botwork"));
    // "😀" is two UTF-16 code units and four bytes: `Double` in the call starts
    // at character 16 on line 2.
    let text = "Double |x| { Return |x * 2| }\nPair |a| |b| { Return |a| }\nPair |\"😀\"| |@{ Double |2| }|\n";
    assert_eq!(client.open(&uri, text), Vec::<Value>::new());
    let definition = client.at("textDocument/definition", &uri, 2, 16, json!({}));
    assert_eq!(
        definition,
        json!([{"uri": uri, "range": range((0, 0), (0, 10))}])
    );
    // Counted in bytes, character 16 is the `{` of `@{`; counted in code
    // points, character 15 is already `Double`. Both belong to the `Pair` call.
    assert_eq!(
        client.at("textDocument/definition", &uri, 2, 15, json!({})),
        json!([{"uri": uri, "range": range((1, 0), (1, 12))}])
    );
    let references = client.at(
        "textDocument/references",
        &uri,
        0,
        0,
        json!({"context": {"includeDeclaration": true}}),
    );
    assert_eq!(
        references,
        json!([
            {"uri": uri, "range": range((0, 0), (0, 10))},
            {"uri": uri, "range": range((2, 16), (2, 26))},
        ])
    );
    let references = client.at(
        "textDocument/references",
        &uri,
        0,
        0,
        json!({"context": {"includeDeclaration": false}}),
    );
    assert_eq!(references.as_array().unwrap().len(), 1);
    let hover = client.at("textDocument/hover", &uri, 2, 17, json!({}));
    assert_eq!(hover["contents"]["kind"], "markdown");
    assert!(hover["contents"]["value"]
        .as_str()
        .unwrap()
        .contains("Double |x|"));
    assert_eq!(hover["range"], range((2, 16), (2, 26)));
    assert_eq!(
        client.at("textDocument/hover", &uri, 3, 0, json!({})),
        Value::Null
    );
    assert_eq!(client.finish(true), Some(0));
}

#[test]
fn completion_and_hover_cover_statements_and_variables() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::start(directory.path());
    let uri = file_uri(&directory.path().join("main.botwork"));
    client.open(&uri, "|total| = |1|\nLog |total|\n");
    let hover = client.at("textDocument/hover", &uri, 1, 1, json!({}));
    let markdown = hover["contents"]["value"].as_str().unwrap();
    assert!(markdown.contains("Log |value|"), "{markdown}");
    assert_eq!(hover["range"], range((1, 0), (1, 11)));
    // The document no longer parses; completion keeps the last good analysis.
    client.change(&uri, 2, "|total| = |1|\nLog |total|\nLog |to\n");
    let inside = client.at("textDocument/completion", &uri, 2, 7, json!({}));
    let labels: Vec<(&str, i64)> = inside
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["label"].as_str().unwrap(),
                item["kind"].as_i64().unwrap(),
            )
        })
        .collect();
    assert!(labels.contains(&("total", 6)), "{labels:?}");
    assert!(!labels.iter().any(|(label, _)| *label == "Log |value|"));
    let outside = client.at("textDocument/completion", &uri, 2, 0, json!({}));
    let items = outside.as_array().unwrap();
    let log = items
        .iter()
        .find(|item| item["label"] == "Log |value|")
        .expect("Log is offered");
    assert_eq!(log["kind"], 3);
    assert!(items
        .iter()
        .any(|item| item["label"] == "If" && item["kind"] == 14));
    // Navigation also uses the last good analysis while the text is broken.
    let definition = client.at("textDocument/definition", &uri, 1, 6, json!({}));
    assert_eq!(
        definition,
        json!([{"uri": uri, "range": range((0, 1), (0, 6))}])
    );
    assert_eq!(client.finish(true), Some(0));
}

#[test]
fn definitions_reach_imported_module_files() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("lib")).unwrap();
    let module = directory.path().join("lib/math.botwork");
    std::fs::write(&module, "# math\nDouble |x| { Return |x * 2| }\n").unwrap();
    let mut client = Client::start(directory.path());
    let uri = file_uri(&directory.path().join("main.botwork"));
    let text = "Import |\"lib/math.botwork\"| As |m|\nLog |@{ m::Double |2| }|\n";
    assert_eq!(client.open(&uri, text), Vec::<Value>::new());
    let definition = client.at("textDocument/definition", &uri, 1, 9, json!({}));
    assert_eq!(
        definition,
        json!([{"uri": file_uri(&module), "range": range((1, 0), (1, 10))}])
    );
    let import = client.at("textDocument/definition", &uri, 0, 10, json!({}));
    assert_eq!(import[0]["uri"], file_uri(&module));
    // A missing module is a diagnostic at its path.
    let diagnostics = client.change(&uri, 2, "Import |\"lib/none.botwork\"| As |m|\n");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["code"], "BW6001");
    assert_eq!(client.finish(true), Some(0));
}

#[test]
fn unknown_requests_fail_and_exit_reports_whether_shutdown_came_first() {
    let directory = tempfile::tempdir().unwrap();
    let mut client = Client::start(directory.path());
    let response = client.call("workspace/unknown", json!({}));
    assert_eq!(response["error"]["code"], -32601);
    // A body that is not JSON gets a parse error.
    client.send_raw("{]");
    let error = client.receive();
    assert_eq!(
        (&error["id"], &error["error"]["code"]),
        (&Value::Null, &json!(-32700))
    );
    // Notifications the server does not handle are ignored.
    client.notify("$/cancelRequest", json!({"id": 1}));
    // Requests about documents that were never opened have no result.
    assert_eq!(
        client.at(
            "textDocument/hover",
            "file:///none.botwork",
            0,
            0,
            json!({})
        ),
        Value::Null
    );
    assert_eq!(client.finish(false), Some(1));

    let mut client = Client::start(directory.path());
    client.request("shutdown", Value::Null);
    let late = client.call("textDocument/hover", json!({}));
    assert_eq!(late["error"]["code"], -32600);
    assert_eq!(client.finish(false), Some(0));
}

#[test]
fn the_server_flag_takes_no_other_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_botwork"))
        .args(["--lsp", "--file", "main.botwork"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--lsp"));
}

/// Publications until `uri`'s own, which comes last, by URI.
fn publications(client: &mut Client, uri: &str) -> std::collections::BTreeMap<String, Value> {
    let mut found = std::collections::BTreeMap::new();
    loop {
        let message = client.receive();
        if message["method"] != "textDocument/publishDiagnostics" {
            continue;
        }
        let published = message["params"]["uri"].as_str().unwrap().to_owned();
        found.insert(published.clone(), message["params"]["diagnostics"].clone());
        if published == uri {
            return found;
        }
    }
}

#[test]
fn module_problems_follow_their_importers_and_their_own_documents() {
    let directory = tempfile::tempdir().unwrap();
    let directory = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::create_dir(directory.join("lib")).unwrap();
    let module = directory.join("lib/m.botwork");
    std::fs::write(&module, "Double |x| { Return |x * y| }\n").unwrap();
    let (main, module_uri) = (file_uri(&directory.join("main.botwork")), file_uri(&module));
    let importing = "Import |\"lib/m.botwork\"| As |m|\nLog |@{ m::Double |2| }|\n";
    let mut client = Client::start(&directory);
    let warning = |published: &std::collections::BTreeMap<String, Value>| {
        let diagnostics = published[&module_uri].as_array().unwrap().clone();
        assert!(diagnostics.len() <= 1, "{diagnostics:?}");
        diagnostics.first().map(|diagnostic| {
            (
                diagnostic["code"].as_str().unwrap().to_owned(),
                diagnostic["range"].clone(),
            )
        })
    };
    let expected = Some(("BW2001".to_owned(), range((0, 25), (0, 26))));
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main, "languageId": "botwork", "version": 1, "text": importing}}),
    );
    let published = publications(&mut client, &main);
    assert_eq!(published[&main], json!([]));
    assert_eq!(warning(&published), expected);
    // Dropping the import clears the module's diagnostics; restoring it brings
    // them back.
    client.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 2}, "contentChanges": [{"text": "Log |1|\n"}]}),
    );
    assert_eq!(warning(&publications(&mut client, &main)), None);
    client.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 3}, "contentChanges": [{"text": importing}]}),
    );
    assert_eq!(warning(&publications(&mut client, &main)), expected);
    // A second importer finds the same warning, which is published once and
    // stays while either importer is open.
    let other = file_uri(&directory.join("other.botwork"));
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": other, "languageId": "botwork", "version": 1, "text": importing}}),
    );
    assert_eq!(warning(&publications(&mut client, &other)), expected);
    let close = |client: &mut Client, uri: &str| {
        client.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
        let published = publications(client, uri);
        assert_eq!(published[uri], json!([]));
        published
    };
    assert_eq!(warning(&close(&mut client, &main)), expected);
    // With no importer open, nothing reaches the module.
    assert_eq!(warning(&close(&mut client, &other)), None);
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main, "languageId": "botwork", "version": 4, "text": importing}}),
    );
    let published = publications(&mut client, &main);
    assert_eq!(warning(&published), expected);
    // An open module shows its own text's problems, not the file on disk's.
    let fixed = "Double |x| { Return |x * 2| }\n";
    let diagnostics = client.open(&module_uri, fixed);
    assert_eq!(diagnostics, Vec::<Value>::new());
    // Closed again, it shows what the importer finds on disk.
    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": module_uri}}),
    );
    assert_eq!(warning(&publications(&mut client, &module_uri)), expected);
    // Saving the fix re-analyzes the importer, which no longer finds it.
    client.open(&module_uri, fixed);
    std::fs::write(&module, fixed).unwrap();
    client.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": module_uri}}),
    );
    let published = publications(&mut client, &module_uri);
    assert_eq!(published[&main], json!([]));
    assert_eq!(warning(&published), None);
    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": module_uri}}),
    );
    assert_eq!(warning(&publications(&mut client, &module_uri)), None);
    assert_eq!(client.finish(true), Some(0));
}
