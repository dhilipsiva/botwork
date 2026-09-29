//! `--lsp`: a Language Server Protocol server on stdin and stdout.
//!
//! Documents are synchronized in full. Diagnostics come from the same parser and
//! checks as `--check`, with the same codes and spans. Problems in an imported
//! module are published for the module's file unless it is open, and cleared
//! when no open document reaches it. While a document does not parse, navigation
//! uses its last version that did. Positions use UTF-16 code units, the
//! protocol default.
use botwork::core::{
    analysis::Severity,
    format::SourceKind,
    language::{Analysis, CompletionKind, Language, Location, Problem},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    io::{self, BufRead, Write},
    path::Path,
};

struct Document {
    uri: String,
    text: String,
    /// The file name the analysis uses: the path of a `file:` URI.
    name: String,
    version: Value,
    analysis: Analysis,
    /// The last analysis that parsed, and its text, for navigation.
    navigation: Option<(String, Analysis)>,
}

/// Read one framed message; None at the end of input, and Null when the body
/// is not JSON.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = message.to_string();
    write!(output, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    output.flush()
}

/// The byte offset of an LSP position (UTF-16 code units), clamped to the text.
fn offset_of(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap_or(0) as usize;
    let character = position["character"].as_u64().unwrap_or(0) as usize;
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(index) => start += index + 1,
            None => return text.len(),
        }
    }
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |index| start + index);
    let mut units = 0;
    for (index, value) in text[start..end].char_indices() {
        if units >= character {
            return start + index;
        }
        units += value.len_utf16();
    }
    end
}

/// The LSP position of a byte offset.
fn position_of(text: &str, offset: usize) -> Value {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |index| index + 1);
    let character: usize = before[start..].chars().map(char::len_utf16).sum();
    json!({"line": line, "character": character})
}

fn range(text: &str, start: usize, end: usize) -> Value {
    json!({"start": position_of(text, start), "end": position_of(text, end)})
}

fn kind_of(uri: &str) -> SourceKind {
    SourceKind::of_path(Path::new(uri))
}

/// A document's file name for analysis: its path for `file:` URIs.
fn name_of(uri: &str) -> String {
    url::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .map_or_else(
            || uri.to_owned(),
            |path| path.to_string_lossy().into_owned(),
        )
}

fn uri_of(path: &str) -> String {
    url::Url::from_file_path(path).map_or_else(|()| path.to_owned(), |url| url.to_string())
}

struct Server {
    language: Language,
    documents: HashMap<String, Document>,
    shutdown: bool,
}

impl Server {
    /// The diagnostics for `uri`: an open document's own problems, or else the
    /// problems that open documents' analyses found in that file.
    fn diagnostics(&self, uri: &str) -> Value {
        if let Some(document) = self.documents.get(uri) {
            let diagnostics: Vec<Value> = document
                .analysis
                .problems
                .iter()
                .map(|problem| diagnostic(&document.text, problem))
                .collect();
            return json!({"uri": uri, "version": document.version, "diagnostics": diagnostics});
        }
        let path = name_of(uri);
        let mut diagnostics = Vec::new();
        let mut uris: Vec<&String> = self.documents.keys().collect();
        uris.sort();
        for file in uris
            .into_iter()
            .filter_map(|uri| self.documents[uri].analysis.related.get(&path))
        {
            for problem in &file.problems {
                let value = diagnostic(&file.text, problem);
                if !diagnostics.contains(&value) {
                    diagnostics.push(value);
                }
            }
        }
        json!({"uri": uri, "diagnostics": diagnostics})
    }

    /// The URI of a file: its open document's, or its `file:` URI.
    fn uri_for(&self, path: &str) -> String {
        self.documents
            .values()
            .find(|document| document.name == path)
            .map_or_else(|| uri_of(path), |document| document.uri.clone())
    }

    /// The other files whose problems a document's analysis found.
    fn related(&self, uri: &str) -> BTreeSet<String> {
        self.documents
            .get(uri)
            .map_or_else(BTreeSet::new, |document| {
                document
                    .analysis
                    .related
                    .keys()
                    .map(|path| self.uri_for(path))
                    .collect()
            })
    }

    /// Analyze a document's new text; returns the other files whose
    /// diagnostics may have changed.
    fn update(&mut self, uri: &str, text: String, version: Value) -> BTreeSet<String> {
        let mut affected = self.related(uri);
        let name = name_of(uri);
        let analysis = self.language.analyze(&name, &text, kind_of(uri));
        let navigation = match self.documents.remove(uri) {
            _ if analysis.parsed => None,
            Some(previous) if previous.analysis.parsed => Some((previous.text, previous.analysis)),
            Some(previous) => previous.navigation,
            None => None,
        };
        self.documents.insert(
            uri.to_owned(),
            Document {
                uri: uri.to_owned(),
                text,
                name,
                version,
                analysis,
                navigation,
            },
        );
        affected.extend(self.related(uri));
        affected
    }

    /// Diagnostics for each affected file, then for `uri` itself.
    fn publish(&self, affected: &BTreeSet<String>, uri: &str) -> Vec<Value> {
        affected
            .iter()
            .filter(|other| *other != uri)
            .map(String::as_str)
            .chain([uri])
            .map(|uri| {
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": self.diagnostics(uri),
                })
            })
            .collect()
    }

    /// The analysis to navigate with, its text, and the offset of a position.
    fn locate(&self, parameters: &Value) -> Option<(&Document, &str, &Analysis, usize)> {
        let uri = parameters["textDocument"]["uri"].as_str()?;
        let document = self.documents.get(uri)?;
        let (text, analysis) = match &document.navigation {
            Some((text, analysis)) => (text.as_str(), analysis),
            None => (document.text.as_str(), &document.analysis),
        };
        Some((
            document,
            text,
            analysis,
            offset_of(text, &parameters["position"]),
        ))
    }

    /// A location in the document, or in a module file read from disk as the
    /// analysis read it.
    fn location(document: &Document, text: &str, location: &Location) -> Value {
        if location.file.is_empty() || location.file == document.name {
            return json!({"uri": document.uri, "range": range(text, location.start, location.end)});
        }
        let other = std::fs::read_to_string(&location.file).unwrap_or_default();
        json!({"uri": uri_of(&location.file), "range": range(&other, location.start, location.end)})
    }

    fn request(&mut self, method: &str, parameters: &Value) -> Result<Value, (i64, String)> {
        if self.shutdown {
            return Err((-32600, "The server is shutting down".into()));
        }
        match method {
            "initialize" => Ok(json!({
                "capabilities": {
                    "positionEncoding": "utf-16",
                    "textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": false}},
                    "completionProvider": {"triggerCharacters": ["|", "@"]},
                    "hoverProvider": true,
                    "definitionProvider": true,
                    "referencesProvider": true,
                },
                "serverInfo": {"name": "botwork", "version": env!("CARGO_PKG_VERSION")},
            })),
            "shutdown" => {
                self.shutdown = true;
                Ok(Value::Null)
            }
            "textDocument/hover" => Ok(self
                .locate(parameters)
                .and_then(|(_, text, analysis, offset)| {
                    let hover = analysis.hover(&self.language, offset)?;
                    Some(json!({
                        "contents": {"kind": "markdown", "value": hover.markdown},
                        "range": range(text, hover.start, hover.end),
                    }))
                })
                .unwrap_or(Value::Null)),
            "textDocument/definition" => Ok(self
                .locate(parameters)
                .map(|(document, text, analysis, offset)| {
                    Value::Array(
                        analysis
                            .definition(offset)
                            .iter()
                            .map(|location| Self::location(document, text, location))
                            .collect(),
                    )
                })
                .unwrap_or(Value::Null)),
            "textDocument/references" => {
                let declaration = parameters["context"]["includeDeclaration"]
                    .as_bool()
                    .unwrap_or(false);
                Ok(self
                    .locate(parameters)
                    .map(|(document, text, analysis, offset)| {
                        Value::Array(
                            analysis
                                .references(offset, declaration)
                                .iter()
                                .map(|location| Self::location(document, text, location))
                                .collect(),
                        )
                    })
                    .unwrap_or(Value::Null))
            }
            "textDocument/completion" => {
                let Some(uri) = parameters["textDocument"]["uri"].as_str() else {
                    return Ok(Value::Null);
                };
                let Some(document) = self.documents.get(uri) else {
                    return Ok(Value::Null);
                };
                let offset = offset_of(&document.text, &parameters["position"]);
                let analysis = document
                    .navigation
                    .as_ref()
                    .map_or(&document.analysis, |(_, analysis)| analysis);
                let items: Vec<Value> = analysis
                    .completions(&self.language, &document.text, offset)
                    .into_iter()
                    .map(|completion| {
                        json!({
                            "label": completion.label,
                            "kind": match completion.kind {
                                CompletionKind::Keyword => 14,
                                CompletionKind::Statement => 3,
                                CompletionKind::Variable => 6,
                            },
                            "detail": completion.detail,
                        })
                    })
                    .collect();
                Ok(Value::Array(items))
            }
            _ => Err((-32601, format!("Method not found: {method}"))),
        }
    }

    /// Handle a notification; returns messages to send.
    fn notify(&mut self, method: &str, parameters: &Value) -> Vec<Value> {
        let uri = parameters["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let affected = match method {
            "textDocument/didOpen" => {
                let text = parameters["textDocument"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.update(&uri, text, parameters["textDocument"]["version"].clone())
            }
            "textDocument/didChange" => {
                let Some(text) = parameters["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.last())
                    .and_then(|change| change["text"].as_str())
                else {
                    return vec![];
                };
                self.update(
                    &uri,
                    text.to_owned(),
                    parameters["textDocument"]["version"].clone(),
                )
            }
            "textDocument/didClose" => {
                let affected = self.related(&uri);
                self.documents.remove(&uri);
                affected
            }
            // A saved file can change what other documents import from disk.
            "textDocument/didSave" => {
                let mut uris: Vec<String> = self.documents.keys().cloned().collect();
                uris.sort();
                let mut affected = BTreeSet::new();
                for other in uris {
                    let document = &self.documents[&other];
                    let (text, version) = (document.text.clone(), document.version.clone());
                    affected.extend(self.update(&other, text, version));
                    affected.insert(other);
                }
                affected
            }
            _ => return vec![],
        };
        self.publish(&affected, &uri)
    }
}

/// An LSP diagnostic for a problem found in `text`.
fn diagnostic(text: &str, problem: &Problem) -> Value {
    json!({
        "range": range(text, problem.start, problem.end),
        "severity": if problem.severity == Severity::Error { 1 } else { 2 },
        "code": problem.code,
        "source": "botwork",
        "message": if problem.help.is_empty() {
            problem.message.clone()
        } else {
            format!("{}\nhelp: {}", problem.message, problem.help)
        },
    })
}

/// Serve until `exit`; the status is 0 after a `shutdown` request, as the
/// protocol requires, and 1 otherwise.
pub(super) fn run() -> i32 {
    let directory = std::env::current_dir().unwrap_or_else(|_| Path::new("/").to_owned());
    let mut server = Server {
        language: Language::new(directory),
        documents: HashMap::new(),
        shutdown: false,
    };
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    loop {
        let message = match read_message(&mut input) {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => return 1,
        };
        if !message.is_object() {
            let error = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}});
            if write_message(&mut output, &error).is_err() {
                return 1;
            }
            continue;
        }
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let parameters = message.get("params").cloned().unwrap_or(Value::Null);
        match message.get("id").cloned() {
            Some(id) => {
                let response = match server.request(&method, &parameters) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err((code, message)) => {
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
                    }
                };
                if write_message(&mut output, &response).is_err() {
                    return 1;
                }
            }
            None if method == "exit" => return if server.shutdown { 0 } else { 1 },
            None => {
                for message in server.notify(&method, &parameters) {
                    if write_message(&mut output, &message).is_err() {
                        return 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_framed_by_content_length_in_any_letter_case() {
        let body = r#"{"id":1}"#;
        let input = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{body}Content-Length: 3\r\n\r\n{{]}}",
            body.len()
        );
        let mut input = input.as_bytes();
        assert_eq!(read_message(&mut input).unwrap(), Some(json!({"id": 1})));
        assert_eq!(read_message(&mut input).unwrap(), Some(Value::Null));
        assert_eq!(read_message(&mut input).unwrap(), None);
        let missing = read_message(&mut "X: 1\r\n\r\n{}".as_bytes()).unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::InvalidData);
        let mut output = Vec::new();
        write_message(&mut output, &json!({"id": 1})).unwrap();
        assert_eq!(
            output,
            format!("Content-Length: 8\r\n\r\n{body}").as_bytes()
        );
    }

    #[test]
    fn positions_count_utf16_code_units_and_clamp_to_the_text() {
        let text = "a😀b\né\n";
        let at = |line: u64, character: u64| {
            offset_of(text, &json!({"line": line, "character": character}))
        };
        assert_eq!(
            [at(0, 0), at(0, 1), at(0, 3), at(0, 4), at(0, 9)],
            [0, 1, 5, 6, 6]
        );
        // Inside a surrogate pair, the offset moves to the character's end.
        assert_eq!(at(0, 2), 5);
        assert_eq!([at(1, 1), at(2, 0), at(3, 0)], [9, 10, 10]);
        for (offset, line, character) in [
            (0, 0, 0),
            (5, 0, 3),
            (6, 0, 4),
            (7, 1, 0),
            (9, 1, 1),
            (10, 2, 0),
            (99, 2, 0),
        ] {
            assert_eq!(
                position_of(text, offset),
                json!({"line": line, "character": character}),
                "{offset}"
            );
        }
    }

    #[test]
    fn file_uris_name_their_paths_and_other_uris_name_themselves() {
        assert_eq!(name_of("file:///tmp/a%20b.botwork"), "/tmp/a b.botwork");
        assert_eq!(name_of("untitled:Untitled-1"), "untitled:Untitled-1");
        assert_eq!(uri_of("/tmp/a b.botwork"), "file:///tmp/a%20b.botwork");
        assert_eq!(uri_of("relative.botwork"), "relative.botwork");
        assert!(matches!(
            kind_of("file:///x/a.suite.botwork"),
            SourceKind::Suite
        ));
        assert!(matches!(
            kind_of("file:///x/a.dataset.botwork"),
            SourceKind::Dataset
        ));
        assert!(matches!(kind_of("untitled:Untitled-1"), SourceKind::Script));
    }
}
