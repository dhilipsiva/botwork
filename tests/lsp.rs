//! `--lsp` serves the Language Server Protocol on stdin and stdout, with
//! diagnostics, completion, hover, definitions, and references from the shared
//! language analysis.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

/// How long any single message may take; a hung server fails the test.
const WAIT: Duration = Duration::from_secs(20);

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next: i64,
}

impl Client {
    fn start(directory: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_botwork"))
            .arg("--lsp")
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = mpsc::channel();
        std::thread::spawn(move || loop {
            let mut length = None;
            loop {
                let mut line = String::new();
                if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    length = Some(value.parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.expect("a Content-Length header")];
            stdout.read_exact(&mut body).unwrap();
            if sender.send(serde_json::from_slice(&body).unwrap()).is_err() {
                return;
            }
        });
        let mut client = Self {
            child,
            stdin,
            messages,
            next: 0,
        };
        let result = client.request("initialize", json!({"capabilities": {}}));
        let capabilities = &result["capabilities"];
        assert_eq!(capabilities["positionEncoding"], "utf-16");
        assert_eq!(capabilities["textDocumentSync"]["change"], 1);
        for provider in ["hoverProvider", "definitionProvider", "referencesProvider"] {
            assert_eq!(capabilities[provider], true, "{provider}");
        }
        assert!(capabilities["completionProvider"].is_object());
        client.notify("initialized", json!({}));
        client
    }

    fn send(&mut self, message: Value) {
        self.send_raw(&message.to_string());
    }

    fn send_raw(&mut self, body: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn receive(&mut self) -> Value {
        self.messages
            .recv_timeout(WAIT)
            .expect("the server answers in time")
    }

    /// The whole response to a request.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.receive();
            if message["id"] == id {
                return message;
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    /// The next diagnostics published for `uri`.
    fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let message = self.receive();
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return message["params"]["diagnostics"].as_array().unwrap().clone();
            }
        }
    }

    fn open(&mut self, uri: &str, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "botwork", "version": 1, "text": text}}),
        );
        self.diagnostics(uri)
    }

    fn change(&mut self, uri: &str, version: i64, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        );
        self.diagnostics(uri)
    }

    fn at(&mut self, method: &str, uri: &str, line: u32, character: u32, extra: Value) -> Value {
        let mut params = json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        });
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        self.request(method, params)
    }

    /// Shut down and exit; returns the exit status.
    fn finish(mut self, shutdown: bool) -> Option<i32> {
        if shutdown {
            assert_eq!(self.request("shutdown", Value::Null), Value::Null);
        }
        self.notify("exit", Value::Null);
        drop(self.stdin.take());
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            assert!(started.elapsed() < WAIT, "the server did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn range(start: (u32, u32), end: (u32, u32)) -> Value {
    json!({
        "start": {"line": start.0, "character": start.1},
        "end": {"line": end.0, "character": end.1},
    })
}

fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

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
    // An incomplete edit reports the syntax error at the end of the text.
    let broken = client.change(&uri, 2, "Log |1\n");
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(broken[0]["code"], "BW1001");
    assert_eq!(broken[0]["severity"], 1);
    assert_eq!(broken[0]["range"], range((0, 6), (1, 0)));
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
