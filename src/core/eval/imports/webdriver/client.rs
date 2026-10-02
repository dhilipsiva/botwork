//! WebDriver's wire: JSON over HTTP/1.1, one connection a command. A command
//! is bounded in time and size, and a stop drops it mid-flight.
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Body, Bytes},
    header::{CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST},
    Method, Request,
};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::{
    fmt,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

/// The most a response may hold: a full-page screenshot's base64 fits.
pub(super) const MAX_RESPONSE: usize = 32 * 1024 * 1024;

/// A WebDriver server: its address, and the path its endpoints start with.
#[derive(Clone, Debug)]
pub(super) struct Endpoint {
    authority: String,
    prefix: String,
}

/// Why a command failed: the driver's error, or reaching the driver.
#[derive(Debug)]
pub(super) enum Failure {
    /// The driver answered with a W3C error, such as `no such element`.
    Driver { error: String, message: String },
    /// The driver could not be reached or answered outside the protocol.
    Transport(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Driver { error, message } => write!(f, "{error}: {message}"),
            Self::Transport(reason) => f.write_str(reason),
        }
    }
}

impl Endpoint {
    /// A driver's URL: `http`, a host, a port, and an optional path.
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let url = url::Url::parse(text)
            .map_err(|error| format!("The driver URL `{text}` is not a URL: {error}"))?;
        if url.scheme() != "http" {
            return Err(format!(
                "The driver URL `{text}` must use http; WebDriver servers listen on plain HTTP"
            ));
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(format!(
                "The driver URL `{text}` may hold only a host, a port, and a path"
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| format!("The driver URL `{text}` has no host"))?;
        let port = url.port_or_known_default().unwrap_or(80);
        let authority = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        Ok(Self {
            authority,
            prefix: url.path().trim_end_matches('/').to_owned(),
        })
    }

    pub(super) fn loopback(port: u16) -> Self {
        Self {
            authority: format!("127.0.0.1:{port}"),
            prefix: String::new(),
        }
    }

    /// Send a command and return its `value`, within `timeout`.
    pub(super) async fn command(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        match tokio::time::timeout(timeout, self.exchange(method.clone(), path, body)).await {
            Ok(result) => result,
            Err(_) => Err(Failure::Transport(format!(
                "the driver did not answer {method} {path} within {} ms",
                timeout.as_millis()
            ))),
        }
    }

    async fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Failure> {
        let unreachable = |error: &dyn fmt::Display| {
            Failure::Transport(format!(
                "could not reach the driver at http://{}: {error}",
                self.authority
            ))
        };
        let stream = tokio::net::TcpStream::connect(&self.authority)
            .await
            .map_err(|error| unreachable(&error))?;
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_buf_size(65_536)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(|error| unreachable(&error))?;
        let payload = body.map(Value::to_string).unwrap_or_default();
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.prefix))
            .header(HOST, &self.authority)
            .header(CONNECTION, "close");
        if body.is_some() {
            request = request.header(CONTENT_TYPE, "application/json; charset=utf-8");
        }
        let request = request
            .header(CONTENT_LENGTH, payload.len())
            .body(Full::new(Bytes::from(payload)))
            .map_err(|error| Failure::Transport(format!("invalid request: {error}")))?;
        let exchange = async move {
            let response = sender.send_request(request).await.map_err(|error| {
                Failure::Transport(format!("the driver's answer was not HTTP: {error}"))
            })?;
            let status = response.status().as_u16();
            let mut body = response.into_body();
            if body.size_hint().lower() > MAX_RESPONSE as u64 {
                return Err(too_large());
            }
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|error| {
                    Failure::Transport(format!("the driver's answer broke off: {error}"))
                })?;
                if let Some(data) = frame.data_ref() {
                    if bytes.len() + data.len() > MAX_RESPONSE {
                        return Err(too_large());
                    }
                    bytes.extend_from_slice(data);
                }
            }
            Ok((status, bytes))
        };
        tokio::pin!(exchange, connection);
        // The connection drives the exchange and may finish first, once the
        // driver has sent its whole answer and closed.
        let mut connected = true;
        let (status, bytes) = loop {
            tokio::select! {
                result = &mut exchange => break result?,
                result = &mut connection, if connected => {
                    connected = false;
                    result.map_err(|error| Failure::Transport(format!("the driver's connection failed: {error}")))?;
                }
            }
        };
        answer(status, &bytes)
    }
}

fn too_large() -> Failure {
    Failure::Transport(format!(
        "the driver's answer is larger than {} MiB",
        MAX_RESPONSE / 1024 / 1024
    ))
}

/// The `value` of a W3C answer, or the error it reports.
fn answer(status: u16, bytes: &[u8]) -> Result<Value, Failure> {
    let mut document: Value = serde_json::from_slice(bytes).map_err(|error| {
        Failure::Transport(format!(
            "the driver's answer (HTTP {status}) is not JSON: {error}"
        ))
    })?;
    let value = document.get_mut("value").map(Value::take).ok_or_else(|| {
        Failure::Transport(format!(
            "the driver's answer (HTTP {status}) has no `value`"
        ))
    })?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        if status >= 400 {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(Failure::Driver {
                error: error.to_owned(),
                message: bounded(message, 1024),
            });
        }
    }
    if status >= 400 {
        return Err(Failure::Transport(format!(
            "the driver answered HTTP {status} without a WebDriver error"
        )));
    }
    Ok(value)
}

/// At most `limit` bytes of `text`, cut at a character boundary.
pub(super) fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Percent-encode one path segment, so a reference cannot reach another
/// endpoint.
pub(super) fn segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl Endpoint {
    /// Delete a session without a runtime, as a run's end cleans up: best
    /// effort, each step bounded by `timeout`.
    pub(super) fn delete_blocking(&self, session: &str, timeout: Duration) {
        // The answer is awaited, so the browser has closed when this returns.
        let _ = self.request_blocking("DELETE", &format!("/session/{}", segment(session)), timeout);
    }

    /// GET `path` without a runtime and return its `value`, or None when the
    /// driver does not answer within `timeout` or reports an error.
    pub(super) fn get_blocking(&self, path: &str, timeout: Duration) -> Option<Value> {
        self.request_blocking("GET", path, timeout)
    }

    fn request_blocking(&self, method: &str, path: &str, timeout: Duration) -> Option<Value> {
        let address = self.authority.to_socket_addrs().ok()?.next()?;
        let mut stream = TcpStream::connect_timeout(&address, timeout).ok()?;
        stream.set_write_timeout(Some(timeout)).ok()?;
        stream.set_read_timeout(Some(timeout)).ok()?;
        write!(
            stream,
            "{method} {}{path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            self.prefix, self.authority
        )
        .ok()?;
        // Read until the body is complete: a driver may keep the connection
        // open whatever the request asks.
        let deadline = std::time::Instant::now() + timeout;
        let mut response = Vec::new();
        let mut buffer = [0; 65536];
        let (head, body_start) = loop {
            if let Some(split) = response.windows(4).position(|window| window == b"\r\n\r\n") {
                break (
                    String::from_utf8_lossy(&response[..split]).to_ascii_lowercase(),
                    split + 4,
                );
            }
            let count = stream.read(&mut buffer).ok().filter(|count| *count != 0)?;
            response.extend_from_slice(&buffer[..count]);
            if response.len() > MAX_RESPONSE || std::time::Instant::now() >= deadline {
                return None;
            }
        };
        let status = head.split_whitespace().nth(1)?.parse().ok()?;
        let chunked = head.contains("transfer-encoding: chunked");
        let length = head.lines().find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        });
        let complete = |response: &[u8]| {
            let body = &response[body_start..];
            match (length, chunked) {
                (Some(length), _) => body.len() >= length,
                (None, true) => unchunk(body).is_some(),
                (None, false) => false,
            }
        };
        while !complete(&response) {
            match stream.read(&mut buffer) {
                Ok(0) if length.is_none() && !chunked => break,
                Ok(0) => return None,
                Ok(count) => response.extend_from_slice(&buffer[..count]),
                Err(_) => return None,
            }
            if response.len() > MAX_RESPONSE || std::time::Instant::now() >= deadline {
                return None;
            }
        }
        let mut body = response[body_start..].to_vec();
        if let Some(length) = length {
            body.truncate(length);
        } else if chunked {
            body = unchunk(&body)?;
        }
        answer(status, &body).ok()
    }
}

/// The body of a chunked HTTP/1.1 message.
fn unchunk(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line = body.windows(2).position(|window| window == b"\r\n")?;
        let size = std::str::from_utf8(&body[..line])
            .ok()?
            .split(';')
            .next()?
            .trim();
        let size = usize::from_str_radix(size, 16).ok()?;
        body = &body[line + 2..];
        if size == 0 {
            return Some(out);
        }
        out.extend_from_slice(body.get(..size)?);
        body = body.get(size + 2..)?;
    }
}
