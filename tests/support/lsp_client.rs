//! A test client for `botwork --lsp`: it frames messages over the server's
//! pipes and fails a test that waits too long for an answer.
#![allow(dead_code)]
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

/// How long any single message may take; a hung server fails the test.
pub const WAIT: Duration = Duration::from_secs(20);

pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next: i64,
}

impl Client {
    pub fn start(directory: &Path) -> Self {
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
        // Saves tell the server that files other documents import may have changed.
        assert_eq!(
            capabilities["textDocumentSync"]["save"],
            json!({"includeText": false})
        );
        for provider in ["hoverProvider", "definitionProvider", "referencesProvider"] {
            assert_eq!(capabilities[provider], true, "{provider}");
        }
        assert!(capabilities["completionProvider"].is_object());
        assert_eq!(capabilities["renameProvider"]["prepareProvider"], true);
        assert!(capabilities["signatureHelpProvider"]["triggerCharacters"].is_array());
        client.notify("initialized", json!({}));
        client
    }

    pub fn send(&mut self, message: Value) {
        self.send_raw(&message.to_string());
    }

    pub fn send_raw(&mut self, body: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    pub fn receive(&mut self) -> Value {
        self.messages
            .recv_timeout(WAIT)
            .expect("the server answers in time")
    }

    /// The whole response to a request.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
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

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    /// The next diagnostics published for `uri`.
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        loop {
            let message = self.receive();
            if message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
            {
                return message["params"]["diagnostics"].as_array().unwrap().clone();
            }
        }
    }

    pub fn open(&mut self, uri: &str, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "botwork", "version": 1, "text": text}}),
        );
        self.diagnostics(uri)
    }

    pub fn change(&mut self, uri: &str, version: i64, text: &str) -> Vec<Value> {
        self.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
        );
        self.diagnostics(uri)
    }

    pub fn at(
        &mut self,
        method: &str,
        uri: &str,
        line: u32,
        character: u32,
        extra: Value,
    ) -> Value {
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
    pub fn finish(mut self, shutdown: bool) -> Option<i32> {
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

pub fn range(start: (u32, u32), end: (u32, u32)) -> Value {
    json!({
        "start": {"line": start.0, "character": start.1},
        "end": {"line": end.0, "character": end.1},
    })
}

pub fn file_uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}
