//! A fake W3C WebDriver server for tests: it answers each command from a fixed
//! script, records what it was sent, and needs no browser.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub const ELEMENT: &str = "element-6066-11e4-a52e-4f735466cecf";
/// A 1×1 PNG, as a screenshot.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0x7a, 0x5e, 0xab, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

/// One request the fake received.
#[derive(Clone, Debug)]
pub struct Received {
    pub method: String,
    pub path: String,
    pub body: Value,
}

pub struct Fake {
    pub url: String,
    pub received: Arc<Mutex<Vec<Received>>>,
    stop: Arc<AtomicBool>,
}

impl Fake {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let sessions = Arc::new(AtomicUsize::new(0));
        let lookups = Arc::new(AtomicUsize::new(0));
        {
            let (received, stop) = (Arc::clone(&received), Arc::clone(&stop));
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (received, sessions, lookups) = (
                                Arc::clone(&received),
                                Arc::clone(&sessions),
                                Arc::clone(&lookups),
                            );
                            thread::spawn(move || serve(stream, &received, &sessions, &lookups));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
        }
        Self {
            url,
            received,
            stop,
        }
    }

    pub fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    /// The paths of the requests made with `method`.
    pub fn paths(&self, method: &str) -> Vec<String> {
        self.received()
            .into_iter()
            .filter(|request| request.method == method)
            .map(|request| request.path)
            .collect()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn serve(
    stream: TcpStream,
    received: &Mutex<Vec<Received>>,
    sessions: &AtomicUsize,
    lookups: &AtomicUsize,
) {
    stream.set_nonblocking(false).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or_default().to_owned(),
        parts.next().unwrap_or_default().to_owned(),
    );
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header == "\r\n" || header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    received.lock().unwrap().push(Received {
        method: method.clone(),
        path: path.clone(),
        body: body.clone(),
    });
    let (status, value) = answer(&method, &path, &body, sessions, lookups);
    if status == 0 {
        // Never answer, holding the connection open.
        thread::sleep(Duration::from_secs(60));
        return;
    }
    let text = json!({ "value": value }).to_string();
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
}

fn error(error: &str, message: &str) -> (u16, Value) {
    (
        404,
        json!({ "error": error, "message": message, "stacktrace": "" }),
    )
}

fn answer(
    method: &str,
    path: &str,
    body: &Value,
    sessions: &AtomicUsize,
    lookups: &AtomicUsize,
) -> (u16, Value) {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", ["status"]) => (200, json!({ "ready": true, "message": "fake" })),
        ("POST", ["session"]) => {
            let wanted = &body["capabilities"]["alwaysMatch"];
            if wanted["browserName"] == "refused" {
                return (
                    500,
                    json!({ "error": "session not created", "message": "no such browser" }),
                );
            }
            let number = sessions.fetch_add(1, Ordering::SeqCst) + 1;
            // An Appium session names its platform, not a browser.
            let capabilities = if wanted["platformName"].is_string() {
                json!({ "platformName": wanted["platformName"], "automationName": "UiAutomator2" })
            } else {
                json!({ "browserName": "fake", "browserVersion": "1.0" })
            };
            (
                200,
                json!({ "sessionId": format!("s{number}"), "capabilities": capabilities }),
            )
        }
        ("DELETE", ["session", _]) => (200, Value::Null),
        ("POST", ["session", _, "url"]) if body["url"] == "http://hang.test/" => (0, Value::Null),
        ("POST", ["session", _, "url"]) => (200, Value::Null),
        ("GET", ["session", _, "url"]) => (200, json!("http://fake.test/")),
        ("GET", ["session", _, "title"]) => (200, json!("Fake page")),
        ("POST", ["session", _, "element"] | ["session", _, "element", _, "element"]) => {
            match body["value"].as_str() {
                Some("#missing") => error("no such element", "Unable to locate element: #missing"),
                Some(value) => (200, json!({ ELEMENT: format!("e-{value}") })),
                None => error("invalid argument", "no value"),
            }
        }
        ("POST", ["session", _, "elements"] | ["session", _, "element", _, "elements"]) => {
            match body["value"].as_str() {
                // Appears on the third look.
                Some("#late") if lookups.fetch_add(1, Ordering::SeqCst) < 2 => (200, json!([])),
                Some("#never" | "p") => (200, json!([])),
                Some(value) => (
                    200,
                    json!([{ ELEMENT: format!("e-{value}") }, { ELEMENT: format!("e-{value}-2") }]),
                ),
                None => error("invalid argument", "no value"),
            }
        }
        ("POST", ["session", _, "element", _, "click" | "clear" | "value"]) => (200, Value::Null),
        ("GET", ["session", _, "element", element, "text"]) => {
            (200, json!(format!("text of {element}")))
        }
        ("GET", ["session", _, "element", _, "attribute", "missing"]) => (200, Value::Null),
        ("GET", ["session", _, "element", _, "attribute", name]) => {
            (200, json!(format!("value of {name}")))
        }
        ("GET", ["session", _, "element", _, "displayed"]) => (200, json!(true)),
        ("POST", ["session", _, "execute", "sync"]) => match body["script"].as_str() {
            Some("echo") => (200, body["args"].clone()),
            Some("huge") => (200, json!(2147483649i64)),
            Some("nothing") => (200, Value::Null),
            Some(other) => (200, json!(other)),
            None => error("invalid argument", "no script"),
        },
        ("GET", ["session", _, "screenshot"]) => (200, json!(encode(PNG))),
        ("GET", ["session", _, "source"]) => (200, json!("<html><body>fake</body></html>")),
        _ => error("unknown command", path),
    }
}

fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let buffer = chunk
            .iter()
            .enumerate()
            .fold(0u32, |buffer, (index, byte)| {
                buffer | u32::from(*byte) << (16 - 8 * index)
            });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(DIGITS[(buffer >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
