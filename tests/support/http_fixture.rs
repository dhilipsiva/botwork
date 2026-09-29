//! Local HTTP endpoint used by executable documentation and CLI examples.
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
pub struct Fixture {
    pub url: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
type Respond = Box<dyn Fn(&str) -> Vec<u8> + Send>;

fn response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

impl Fixture {
    /// Answer every request with `{"message":"hello"}`; `url` ends in `/hello`.
    pub fn new() -> Self {
        Self::start(
            "/hello",
            Box::new(|_| response("200 OK", b"{\"message\":\"hello\"}")),
        )
    }

    /// Serve `GET /<name>` from the files directly inside `directory`, like
    /// `python3 -m http.server`; other paths return 404. `url` has no path.
    #[allow(dead_code)]
    pub fn serving(directory: std::path::PathBuf) -> Self {
        Self::start(
            "",
            Box::new(move |path| {
                let name = path.strip_prefix('/').unwrap_or(path);
                let file = (!name.is_empty() && !name.contains(['/', '\\']) && name != "..")
                    .then(|| std::fs::read(directory.join(name)).ok())
                    .flatten();
                match file {
                    Some(body) => response("200 OK", &body),
                    None => response("404 Not Found", b"{\"error\":\"not found\"}"),
                }
            }),
        )
    }

    fn start(suffix: &str, respond: Respond) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}{suffix}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let worker = thread::spawn(move || {
            while !cancelled.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let mut bytes = Vec::new();
                        let mut byte = [0];
                        while bytes.len() < 16_384 && stream.read_exact(&mut byte).is_ok() {
                            bytes.push(byte[0]);
                            if bytes.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                        let request = String::from_utf8_lossy(&bytes);
                        let path = request.split(' ').nth(1).unwrap_or("/");
                        let _ = stream.write_all(&respond(path));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("HTTP fixture accept: {error}"),
                }
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
