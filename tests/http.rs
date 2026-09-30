use botwork::core::{
    diagnostic::DiagnosticCode as Code,
    grammar::Literal,
    operation::OperationControl,
    run::{Engine, RunLimits, RunOptions, RunOutcome, RunResult},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn read_request(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> Vec<u8> {
    let mut bytes = Vec::new();
    let end = loop {
        let byte = stream.read_u8().await.unwrap();
        bytes.push(byte);
        assert!(bytes.len() < 65_536);
        if bytes.ends_with(b"\r\n\r\n") {
            break bytes.len();
        }
    };
    let headers = String::from_utf8_lossy(&bytes).to_lowercase();
    let length: usize = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
        .unwrap_or("0")
        .parse()
        .unwrap();
    bytes.resize(end + length, 0);
    stream.read_exact(&mut bytes[end..]).await.unwrap();
    bytes
}
async fn serve(responses: Vec<Vec<u8>>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            recorded.lock().unwrap().push(request);
            stream.write_all(&response).await.unwrap();
        }
        // Retain the listening address so an unwanted retry/redirect cannot hit
        // a different test which reuses the port.
        std::future::pending::<()>().await;
        drop(listener);
    });
    Server {
        url,
        requests,
        task,
    }
}
fn reply(status: u16, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
fn options(url: &str) -> RunOptions {
    RunOptions {
        variables: BTreeMap::from([("url".into(), Literal::String(url.into()))]),
        ..Default::default()
    }
}
async fn run(url: &str, source: &str) -> RunResult {
    // Independent test cases share process-wide transport quotas.
    static CASES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let _case = CASES.acquire().await.unwrap();
    Engine::default()
        .run_source_async("http.botwork", source, options(url))
        .await
}
async fn ok(url: &str, source: &str) {
    let result = run(url, source).await;
    assert!(result.result.is_ok(), "{source}: {:?}", result.result);
}

#[tokio::test]
async fn methods_targets_unicode_bodies_duplicate_headers_and_status_results() {
    for method in [
        "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "CUSTOM",
    ] {
        let body = if method == "HEAD" {
            b"".as_slice()
        } else {
            "é".as_bytes()
        };
        let server = serve(vec![reply(422, "X-Value: one\r\nx-value: two\r\n", body)]).await;
        let source = format!(
            r#"
|r| = HTTP Request |"{method}"| To |url| Options |{{"body": "input", "headers": {{"x-test": ["a", "b"]}}}}|
Assert |r.status| Equals |422|
Assert |r.success| Equals |false|
Assert |r.body| Equals |"{}"|
Assert |r.headers["x-value"]| Equals |["one", "two"]|
Assert |r.redirects| Equals |0|
Assert |r.attempts| Equals |1|
Assert |r.url| Equals |url|
Assert |@{{ Length Of |r| }}| Equals |7|
"#,
            std::str::from_utf8(body).unwrap()
        );
        ok(&format!("{}/path?q=one%20two", server.url), &source).await;
        let requests = server.requests.lock().unwrap();
        let request = String::from_utf8_lossy(&requests[0]);
        assert!(
            request.starts_with(&format!("{method} /path?q=one%20two HTTP/1.1\r\n")),
            "{request}"
        );
        assert!(request.contains("x-test: a\r\nx-test: b\r\n"));
        assert!(request.ends_with("\r\n\r\ninput"));
    }
}

#[tokio::test]
async fn binary_calls_preserve_raw_body_and_header_bytes_and_text_rejects_invalid_utf8() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nX-Raw: \xff\r\n\r\n\x00\xff\x0a".to_vec();
    let server = serve(vec![raw.clone(), raw]).await;
    ok(
        &server.url,
        r#"
|r| = HTTP Binary Request |"POST"| To |url| Options |{"body": [0, 255, 10]}|
Assert |r.body| Equals |[0, 255, 10]|
Assert |r.headers["x-raw"]| Equals |[[255]]|
Assert |r.success|
"#,
    )
    .await;
    assert!(server.requests.lock().unwrap()[0].ends_with(&[0, 255, 10]));
    assert_eq!(
        run(&server.url, "HTTP Request |\"GET\"| To |url|")
            .await
            .result
            .unwrap_err()
            .code(),
        Code::IncompatibleType
    );
    let server = serve(vec![reply(200, "", &[255])]).await;
    assert_eq!(
        run(&server.url, "HTTP Request |\"GET\"| To |url|")
            .await
            .result
            .unwrap_err()
            .code(),
        Code::IncompatibleType
    );
}

#[tokio::test]
async fn redirects_default_off_and_follow_relative_locations_with_bounded_counts() {
    let server = serve(vec![reply(302, "Location: /next\r\n", b"redirect")]).await;
    ok(
        &server.url,
        r#"|r| = HTTP Request |"GET"| To |url|
Assert |r.status| Equals |302|
Assert |r.body| Equals |"redirect"|
Assert |r.attempts| Equals |1|"#,
    )
    .await;
    let server = serve(vec![
        reply(301, "Location: /next#fragment\r\n", b""),
        reply(200, "", b"done"),
    ])
    .await;
    ok(&server.url, r#"|r| = HTTP Request |"POST"| To |url| Options |{"body": "secret", "headers": {"content-type": "text/plain", "x-keep": "yes"}, "redirects": "same-origin"}|
Assert |r.status| Equals |200|
Assert |r.redirects| Equals |1|
Assert |r.attempts| Equals |2|
Assert |r.body| Equals |"done"|
Assert |r.url| Equals |url + "/next"|"#).await;
    let requests = server.requests.lock().unwrap();
    let next = String::from_utf8_lossy(&requests[1]);
    assert!(next.starts_with("GET /next HTTP/1.1"));
    assert!(!next.contains("secret") && !next.contains("content-type"));
    assert!(next.contains("x-keep: yes"));
}

#[tokio::test]
async fn redirects_preserve_307_bodies_rewrite_303_and_reject_loops() {
    for (status, next_method, body) in [
        (307, "POST", "payload"),
        (308, "POST", "payload"),
        (303, "GET", ""),
    ] {
        let server = serve(vec![
            reply(status, "Location: /next\r\n", b""),
            reply(200, "", b""),
        ])
        .await;
        ok(&server.url, r#"HTTP Request |"POST"| To |url| Options |{"body": "payload", "redirects": "same-origin"}|"#).await;
        let requests = server.requests.lock().unwrap();
        let next = String::from_utf8_lossy(&requests[1]);
        assert!(next.starts_with(&format!("{next_method} /next HTTP/1.1")));
        assert!(next.ends_with(&format!("\r\n\r\n{body}")));
    }
    let server = serve(vec![reply(302, "Location: /\r\n", b""); 3]).await;
    let result = run(&server.url, r#"HTTP Request |"GET"| To |url| Options |{"redirects": "same-origin", "max_redirects": 2}|"#).await;
    assert_eq!(result.result.unwrap_err().code(), Code::Native);
    assert_eq!(server.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn cross_origin_redirects_require_opt_in_and_strip_all_user_headers() {
    let destination = serve(vec![reply(200, "", b"safe")]).await;
    for policy in ["same-origin", "any-origin"] {
        let server = serve(vec![reply(
            302,
            &format!("Location: {}/next\r\n", destination.url),
            b"",
        )])
        .await;
        let source = format!(
            r#"HTTP Request |"GET"| To |url| Options |{{"headers": {{"authorization": "secret", "cookie": "session", "x-api-key": "secret"}}, "redirects": "{policy}"}}|"#
        );
        let result = run(&server.url, &source).await;
        if policy == "same-origin" {
            assert_eq!(result.result.unwrap_err().code(), Code::Native);
        } else {
            assert!(result.result.is_ok(), "{:?}", result.result);
        }
    }
    let requests = destination.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let next = String::from_utf8_lossy(&requests[0]);
    for field in ["authorization", "cookie", "x-api-key", "secret"] {
        assert!(!next.contains(field), "{next}");
    }
}

#[tokio::test]
async fn retries_default_off_exhaust_normally_and_count_every_request() {
    let server = serve(vec![reply(503, "", b"busy")]).await;
    ok(
        &server.url,
        r#"|r| = HTTP Request |"GET"| To |url|
Assert |r.status| Equals |503|
Assert |r.attempts| Equals |1|"#,
    )
    .await;
    let server = serve(vec![
        reply(302, "Location: /next\r\n", b""),
        reply(429, "", b"busy"),
        reply(503, "", b"busy"),
        reply(200, "", b"ready"),
    ])
    .await;
    ok(&server.url, r#"|r| = HTTP Request |"GET"| To |url| Options |{"redirects": "same-origin", "max_retries": 2, "retry_delay_ms": 0}|
Assert |r.attempts| Equals |4|
Assert |r.redirects| Equals |1|
Assert |r.body| Equals |"ready"|"#).await;
    let server = serve(vec![reply(504, "", b"busy"); 3]).await;
    ok(
        &server.url,
        r#"|r| = HTTP Request |"GET"| To |url| Options |{"max_retries": 2, "retry_delay_ms": 0}|
Assert |r.attempts| Equals |3|
Assert |r.status| Equals |504|"#,
    )
    .await;
}

#[tokio::test]
async fn invalid_options_headers_and_unsafe_retries_fail_before_connecting() {
    let server = serve(vec![reply(200, "", b"")]).await;
    for options in [
        r#"{"typo": 1}"#,
        r#"{"timeout_ms": -1}"#,
        r#"{"timeout_ms": 1.5}"#,
        r#"{"max_body_bytes": 1048577}"#,
        r#"{"max_headers": 129}"#,
        r#"{"max_header_bytes": 65537}"#,
        r#"{"max_retries": 6}"#,
        r#"{"max_redirects": 11}"#,
        r#"{"retry_delay_ms": 60001}"#,
        r#"{"redirects": true}"#,
        r#"{"redirects": "always"}"#,
        r#"{"headers": []}"#,
        r#"{"headers": {"Host": "evil"}}"#,
        r#"{"headers": {"content-length": "9"}}"#,
        r#"{"headers": {"transfer-encoding": "chunked"}}"#,
        r#"{"headers": {"connection": "upgrade"}}"#,
        r#"{"headers": {"bad name": "x"}}"#,
        r#"{"headers": {"x": [1]}}"#,
        r#"{"body": [1]}"#,
        r#"{"max_retries": 1}"#,
        r#"{"ca_pem": "invalid"}"#,
    ] {
        let source = format!("HTTP Request |\"POST\"| To |url| Options |{options}|");
        assert_eq!(
            run(&server.url, &source).await.result.unwrap_err().code(),
            Code::IncompatibleType,
            "{source}"
        );
    }
    for method in ["CONNECT", "BAD METHOD", ""] {
        let source = format!("HTTP Request |\"{method}\"| To |url|");
        assert_eq!(
            run(&server.url, &source).await.result.unwrap_err().code(),
            Code::IncompatibleType
        );
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_urls_and_binary_bytes_are_rejected() {
    for url in [
        "ftp://localhost/",
        "http://user:password@localhost",
        "http:///",
        "relative",
        "http://local host/",
        "http://localhost/\n",
    ] {
        assert_eq!(
            run(url, r#"HTTP Request |"GET"| To |url|"#)
                .await
                .result
                .unwrap_err()
                .code(),
            Code::IncompatibleType,
            "{url}"
        );
    }
    for body in ["[256]", "[-1]", "[1.0]", "[true]", "\"text\""] {
        let source =
            format!("HTTP Binary Request |\"POST\"| To |url| Options |{{\"body\": {body}}}|");
        assert_eq!(
            run("http://127.0.0.1:1/", &source)
                .await
                .result
                .unwrap_err()
                .code(),
            Code::IncompatibleType
        );
    }
}

#[tokio::test]
async fn response_limits_apply_to_length_chunked_bytes_headers_and_trailers() {
    let cases = [
        (reply(200, "", b"abc"), r#"{"max_body_bytes": 2}"#),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n".to_vec(),
            r#"{"max_body_bytes": 2}"#,
        ),
        (
            reply(200, "X-Value: abcd\r\n", b""),
            r#"{"max_header_bytes": 20}"#,
        ),
        (reply(200, "X-Value: a\r\n", b""), r#"{"max_headers": 1}"#),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-Value: abcd\r\n\r\n"
                .to_vec(),
            r#"{"max_header_bytes": 30}"#,
        ),
    ];
    for (response, options) in cases {
        let server = serve(vec![response]).await;
        let source = format!("HTTP Request |\"GET\"| To |url| Options |{options}|");
        assert_eq!(
            run(&server.url, &source).await.result.unwrap_err().code(),
            Code::ResourceLimit,
            "{source}"
        );
    }
    let server = serve(vec![b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\nX-Trailer: done\r\n\r\n".to_vec()]).await;
    ok(
        &server.url,
        r#"|r| = HTTP Request |"GET"| To |url|
Assert |r.body| Equals |"hi"|
Assert |r.headers["x-trailer"]| Equals |["done"]|"#,
    )
    .await;
}

#[tokio::test]
async fn partial_bodies_malformed_protocol_and_transport_errors_are_failures_without_retries() {
    for response in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort".to_vec(),
        b"not http\r\n\r\n".to_vec(),
        b"HTTP/1.1 101 Switching Protocols\r\n\r\n".to_vec(),
    ] {
        let server = serve(vec![response]).await;
        let result = run(
            &server.url,
            r#"HTTP Request |"GET"| To |url| Options |{"max_retries": 2, "timeout_ms": 10000}|"#,
        )
        .await;
        assert_eq!(result.result.unwrap_err().code(), Code::Native);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    // Keep the refused port owned: releasing an ephemeral listener lets another
    // concurrently running fixture claim it and receive this request. macOS
    // drops connections to a bound socket that is not listening instead of
    // refusing them, so there the port is released; it assigns ephemeral
    // ports at random, which makes another fixture claiming it unlikely.
    let reserved = tokio::net::TcpSocket::new_v4().unwrap();
    reserved.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let url = format!("http://{}", reserved.local_addr().unwrap());
    #[cfg(target_os = "macos")]
    drop(reserved);
    assert_eq!(
        run(&url, r#"HTTP Request |"GET"| To |url|"#)
            .await
            .result
            .unwrap_err()
            .code(),
        Code::Native
    );
}

#[tokio::test]
async fn output_and_temporary_admission_precede_network_effects() {
    let server = serve(vec![reply(200, "", b"")]).await;
    for limits in [
        RunLimits {
            values: botwork::core::value_limits::ValueLimits {
                string_bytes: 100,
                ..Default::default()
            },
            ..Default::default()
        },
        RunLimits {
            temporaries: botwork::core::run::TemporaryLimits {
                payload_bytes: 2 * 1024 * 1024,
                ..Default::default()
            },
            ..Default::default()
        },
    ] {
        let result = Engine::default()
            .run_source_async(
                "limit",
                r#"HTTP Request |"GET"| To |url|"#,
                RunOptions {
                    limits,
                    ..options(&server.url)
                },
            )
            .await;
        assert_eq!(
            result.outcome(),
            RunOutcome::LimitExceeded,
            "{:?}",
            result.result
        );
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancel_timeout_and_drop_close_pending_sockets_and_finally_runs() {
    for stop in ["cancel", "timeout", "drop", "body-timeout", "tls-timeout"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "{}://{}",
            if stop == "tls-timeout" {
                "https"
            } else {
                "http"
            },
            listener.local_addr().unwrap()
        );
        let (started, ready) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            if stop != "tls-timeout" {
                read_request(&mut stream).await;
            }
            if stop == "body-timeout" {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx")
                    .await
                    .unwrap();
            }
            started.send(()).unwrap();
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes).await;
        });
        let directory = tempfile::tempdir().unwrap();
        let control = OperationControl::default();
        let engine = Engine::default();
        let source = r#"Try { HTTP Request |"GET"| To |url| Options |{"timeout_ms": 60000}| }
Catch |error| { Write File |"caught"| Text |"bad"| }
Finally { Write File |"finally"| Text |"done"| }"#;
        let mut pending = Box::pin(engine.run_source_async(
            "stop",
            source,
            RunOptions {
                working_directory: Some(directory.path().into()),
                control: control.clone(),
                ..options(&url)
            },
        ));
        tokio::select! { result = &mut pending => panic!("premature result: {:?}", result.result), _ = ready => {} }
        if stop == "drop" {
            drop(pending);
        } else {
            if stop == "cancel" {
                control.cancel();
            } else {
                // Network readiness is observed before moving the test clock.
                // Resume before Finally's real filesystem work can start.
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(61)).await;
                tokio::time::resume();
            }
            let result = pending.await;
            assert_eq!(
                result.result.unwrap_err().code(),
                if stop == "cancel" {
                    Code::Cancelled
                } else {
                    Code::Timeout
                }
            );
            assert!(!directory.path().join("caught").exists());
            assert_eq!(
                std::fs::read(directory.path().join("finally")).unwrap(),
                b"done"
            );
        }
        tokio::time::timeout(Duration::from_secs(2), peer)
            .await
            .expect("socket closed")
            .unwrap();
    }
}

#[tokio::test]
async fn inherited_deadline_and_retry_delay_share_end_to_end_budget() {
    let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
    let url = format!("http://{}", listener.local_addr().unwrap());
    let accepting = listener.clone();
    let (closed, ready) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = accepting.accept().await.unwrap();
        read_request(&mut stream).await;
        stream.write_all(&reply(503, "", b"")).await.unwrap();
        let mut data = Vec::new();
        stream.read_to_end(&mut data).await.unwrap();
        // The client releases this completed exchange before its retry delay.
        closed.send(()).unwrap();
    });
    let engine = Engine::default();
    let mut pending = Box::pin(engine.run_source_async("deadline", r#"HTTP Request |"GET"| To |url| Options |{"timeout_ms": 600000, "max_retries": 1, "retry_delay_ms": 60000}|"#, RunOptions {
        control: OperationControl::default().child(Some(tokio::time::Instant::now() + Duration::from_secs(60))), ..options(&url)
    }));
    tokio::select! { result = &mut pending => panic!("retry delay ended before handshake: {:?}", result.result), _ = ready => {} }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    let result = pending.await;
    assert_eq!(result.result.unwrap_err().code(), Code::Timeout);
    peer.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err(),
        "no retry after the inherited deadline"
    );
    let server = serve(vec![reply(200, "", b"")]).await;
    assert_eq!(
        run(
            &server.url,
            r#"HTTP Request |"GET"| To |url| Options |{"timeout_ms": 0}|"#
        )
        .await
        .result
        .unwrap_err()
        .code(),
        Code::Timeout
    );
    assert!(server.requests.lock().unwrap().is_empty());
}

async fn tls_server(response: Vec<u8>) -> Server {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let cert = CertificateDer::from_pem_slice(include_bytes!("http-fixtures/server.pem")).unwrap();
    let key = PrivateKeyDer::from_pem_slice(include_bytes!("http-fixtures/server.key")).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        if let Ok(mut stream) = tokio_rustls::TlsAcceptor::from(Arc::new(config))
            .accept(stream)
            .await
        {
            let request = read_request(&mut stream).await;
            recorded.lock().unwrap().push(request);
            stream.write_all(&response).await.unwrap();
            let _ = stream.shutdown().await;
        }
    });
    Server {
        url,
        requests,
        task,
    }
}
#[tokio::test]
async fn tls_verifies_roots_and_hostnames_and_forbids_downgrade() {
    for (custom_ca, correct_name) in [(false, true), (true, false), (true, true)] {
        let server = tls_server(reply(200, "", b"secure")).await;
        let url = if correct_name {
            server.url.clone()
        } else {
            server.url.replace("localhost", "127.0.0.1")
        };
        let mut options = options(&url);
        options.variables.insert(
            "ca".into(),
            Literal::String(include_str!("http-fixtures/ca.pem").into()),
        );
        let source = if custom_ca {
            r#"|r| = HTTP Request |"GET"| To |url| Options |{"ca_pem": ca}|
Assert |r.body| Equals |"secure"|"#
        } else {
            r#"HTTP Request |"GET"| To |url|"#
        };
        let result = Engine::default()
            .run_source_async("tls", source, options)
            .await;
        if custom_ca && correct_name {
            assert!(result.result.is_ok(), "{:?}", result.result);
        } else {
            assert_eq!(result.result.unwrap_err().code(), Code::Native);
        }
    }
    let destination = serve(vec![reply(200, "", b"")]).await;
    let server = tls_server(reply(
        302,
        &format!("Location: {}\r\n", destination.url),
        b"",
    ))
    .await;
    let mut options = options(&server.url);
    options.variables.insert(
        "ca".into(),
        Literal::String(include_str!("http-fixtures/ca.pem").into()),
    );
    let result = Engine::default()
        .run_source_async(
            "downgrade",
            r#"HTTP Request |"GET"| To |url| Options |{"ca_pem": ca, "redirects": "any-origin"}|"#,
            options,
        )
        .await;
    assert_eq!(result.result.unwrap_err().code(), Code::Native);
    assert!(destination.requests.lock().unwrap().is_empty());
}

#[test]
fn sync_rejection_precedes_argument_effects_and_metadata_preserves_overrides() {
    let directory = tempfile::tempdir().unwrap();
    let result = Engine::default().run_source(
        "sync",
        r#"HTTP Request |@{ Write File |"effect"| Text |"bad"| }| To |"http://localhost"|"#,
        RunOptions {
            working_directory: Some(directory.path().into()),
            ..Default::default()
        },
    );
    assert_eq!(result.result.unwrap_err().code(), Code::AsyncRuntime);
    assert!(!directory.path().join("effect").exists());
    use botwork::core::{
        ast::Program,
        eval::{evaluate_program_detailed, Context},
    };
    let mut context = Context::default();
    context
        .register_native("HTTP Request |method| To |url|", |_| Ok(Literal::Int(42)))
        .unwrap();
    context.init_statements();
    context.init_statements();
    assert_eq!(context.statement_signatures().len(), 100);
    let help = context
        .statement_signature("HTTP Binary Request |m| To |u| Options |o|")
        .unwrap()
        .unwrap()
        .help();
    for text in [
        "method: String",
        "url: String",
        "options: Map",
        "returns: Map",
        "BW3003",
        "BW4002",
        "BW8001",
        "BW5001",
        "BW5002",
        "BW5003",
    ] {
        assert!(help.contains(text), "{help}");
    }
    assert!(matches!(
        evaluate_program_detailed(
            &Program::parse("override", "HTTP Request |1| To |2|").unwrap(),
            &mut context
        )
        .unwrap(),
        Literal::Int(42)
    ));
}

#[test]
fn missing_io_driver_is_a_diagnostic() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let result = runtime.block_on(run(
        "http://127.0.0.1:1/",
        r#"HTTP Request |"GET"| To |url|"#,
    ));
    assert_eq!(result.result.unwrap_err().code(), Code::AsyncRuntime);
}

#[tokio::test]
async fn host_supplied_header_controls_are_rejected_before_network_effects() {
    let server = serve(vec![reply(200, "", b"")]).await;
    for value in ["a\r\nb", "a\nb", "a\0b"] {
        let mut options = options(&server.url);
        options.variables.insert(
            "opts".into(),
            Literal::Map(std::collections::HashMap::from([(
                "headers".into(),
                Literal::Map(std::collections::HashMap::from([(
                    "x-test".into(),
                    Literal::String(value.into()),
                )])),
            )])),
        );
        let result = Engine::default()
            .run_source_async(
                "controls",
                r#"HTTP Request |"GET"| To |url| Options |opts|"#,
                options,
            )
            .await;
        assert_eq!(result.result.unwrap_err().code(), Code::IncompatibleType);
    }
    assert!(server.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn empty_response_and_exact_body_boundary_succeed() {
    let server = serve(vec![
        b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
        reply(200, "", b"abc"),
    ])
    .await;
    ok(&server.url, r#"|r| = HTTP Binary Request |"GET"| To |url| Options |{"max_body_bytes": 0, "max_headers": 0, "max_header_bytes": 0}|
Assert |r.body| Equals |[]|
Assert |r.headers| Equals |{}|
Assert |r.success|
|r| = HTTP Request |"GET"| To |url| Options |{"max_body_bytes": 3}|
Assert |r.body| Equals |"abc"|"#).await;
}

#[tokio::test]
async fn redirect_policy_rejects_body_forwarding_and_ambiguous_locations() {
    let destination = serve(vec![reply(200, "", b"")]).await;
    for location in [
        format!("Location: {}\r\n", destination.url),
        "Location: /a\r\nLocation: /b\r\n".into(),
        "Location: ftp://localhost/file\r\n".into(),
    ] {
        let server = serve(vec![reply(307, &location, b"")]).await;
        let error = run(&server.url, r#"HTTP Request |"POST"| To |url| Options |{"body": "private", "redirects": "any-origin"}|"#).await.result.unwrap_err();
        assert!(matches!(
            error.code(),
            Code::Native | Code::IncompatibleType
        ));
    }
    assert!(destination.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn catchable_transport_failure_preserves_destination_and_call_context() {
    let server = serve(vec![reply(200, "", &[255])]).await;
    ok(
        &server.url,
        r#"|r| = |42|
Try { |r| = HTTP Request |"GET"| To |url| }
Catch |error| { Assert |error.code| Equals |"BW3003"| }
Assert |r| Equals |42|"#,
    )
    .await;
    let server = serve(vec![b"bad HTTP\r\n\r\n".to_vec()]).await;
    let error = run(
        &server.url,
        r#"Fetch { HTTP Request |"GET"| To |url| }
Fetch"#,
    )
    .await
    .result
    .unwrap_err();
    assert_eq!(error.code(), Code::Native);
    let rendered = error.to_string();
    assert!(rendered.contains("http.botwork"), "{rendered}");
}

#[tokio::test]
async fn invalid_text_in_intermediate_responses_is_not_retried() {
    let server = serve(vec![reply(503, "", &[255])]).await;
    let result = run(
        &server.url,
        r#"HTTP Request |"GET"| To |url| Options |{"max_retries": 1, "timeout_ms": 1000}|"#,
    )
    .await;
    assert_eq!(result.result.unwrap_err().code(), Code::IncompatibleType);
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn get_and_head_bodies_cannot_be_retried() {
    let server = serve(vec![reply(200, "", b"")]).await;
    for method in ["GET", "HEAD"] {
        let source = format!(
            r#"HTTP Request |"{method}"| To |url| Options |{{"body": "payload", "max_retries": 1}}|"#
        );
        assert_eq!(
            run(&server.url, &source).await.result.unwrap_err().code(),
            Code::IncompatibleType
        );
    }
    assert!(server.requests.lock().unwrap().is_empty());
}
