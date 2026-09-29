//! Runs share only immutable parsed modules. Variables, module state,
//! environment overlays, working directories, HTTP connections, and records
//! belong to each run, including runs that execute at the same time.
use botwork::core::{
    grammar::Literal,
    report::RecordOptions,
    run::{Engine, RunOptions, RunResult},
    syntax_limits::SyntaxLimits,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

fn options(directory: &Path) -> RunOptions {
    RunOptions {
        working_directory: Some(directory.to_path_buf()),
        inherit_environment: false,
        record: Some(RecordOptions::default()),
        ..RunOptions::default()
    }
}

fn logs(result: &RunResult) -> Vec<String> {
    result
        .record
        .as_ref()
        .unwrap()
        .logs
        .iter()
        .map(|log| log.text.clone())
        .collect()
}

#[test]
fn module_state_is_built_per_run_while_the_parse_is_shared() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("mode.botwork"),
        "Log |\"initializing\"|\n|mode| = Get Environment Variable |\"MODE\"|\nMode { Return |mode| }\n",
    )
    .unwrap();
    let engine = Engine::default();
    let source = "Import |\"mode.botwork\"| As |settings|\nLog |@{ settings::Mode }|";
    for mode in ["a", "b", "c"] {
        let mut run = options(directory.path());
        run.environment = BTreeMap::from([(OsString::from("MODE"), Some(OsString::from(mode)))]);
        let result = engine.run_source("main.botwork", source, run);
        assert!(result.result.is_ok(), "{:?}", result.result);
        assert_eq!(
            logs(&result),
            ["initializing", mode],
            "each run initializes the module"
        );
    }
    let statistics = engine.compiled_modules().statistics();
    assert_eq!(
        (statistics.misses, statistics.hits, statistics.modules),
        (1, 2, 1)
    );
}

#[test]
fn changed_modules_and_tighter_limits_are_never_served_from_the_cache() {
    let directory = tempfile::tempdir().unwrap();
    let module = directory.path().join("value.botwork");
    let engine = Engine::default();
    let source = "Import |\"value.botwork\"| As |value|\nLog |@{ value::Get }|";
    fs::write(&module, "Get { Return |1| }\n").unwrap();
    let first = engine.run_source("main.botwork", source, options(directory.path()));
    assert_eq!(logs(&first), ["1"]);
    fs::write(&module, "Get { Return |2| }\n").unwrap();
    let second = engine.run_source("main.botwork", source, options(directory.path()));
    assert_eq!(logs(&second), ["2"], "the changed text is parsed again");
    // Deep nesting that only the default limits accept.
    fs::write(&module, "Get { Return |((((1))))| }\n").unwrap();
    let accepted = engine.run_source("main.botwork", source, options(directory.path()));
    assert_eq!(logs(&accepted), ["1"]);
    let mut tight = options(directory.path());
    tight.limits.syntax = SyntaxLimits {
        nesting: 3,
        ..SyntaxLimits::default()
    };
    let rejected = engine.run_source("main.botwork", source, tight);
    let error = rejected
        .result
        .expect_err("tighter limits reject the module");
    assert!(error.to_string().contains("nesting"), "{error}");
    assert_eq!(engine.compiled_modules().statistics().hits, 0);
}

#[tokio::test]
async fn concurrent_runs_keep_variables_environment_directories_and_records_apart() {
    let engine = Arc::new(Engine::default());
    let directories: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let current = std::env::current_dir().unwrap();
    let source = r#"|name| = |name + "!"|
Write File |"owned.txt"| Text |name|
Sleep |20|
Log |name|
Log |@{ Get Environment Variable |"ROLE"| }|
Log |@{ Read File |"owned.txt"| }|
"#;
    let runs = directories.iter().enumerate().map(|(index, directory)| {
        let engine = Arc::clone(&engine);
        let mut run = options(directory.path());
        run.variables =
            BTreeMap::from([("name".to_owned(), Literal::String(format!("run{index}")))]);
        run.environment = BTreeMap::from([(
            OsString::from("ROLE"),
            Some(OsString::from(format!("role{index}"))),
        )]);
        async move { engine.run_source_async("shared.botwork", source, run).await }
    });
    let results = futures_join(runs).await;
    for (index, result) in results.iter().enumerate() {
        assert!(result.result.is_ok(), "{:?}", result.result);
        assert_eq!(
            logs(result),
            [
                format!("run{index}!"),
                format!("role{index}"),
                format!("run{index}!")
            ],
            "run {index} sees only its own state"
        );
        assert_eq!(
            fs::read_to_string(directories[index].path().join("owned.txt")).unwrap(),
            format!("run{index}!")
        );
    }
    assert_eq!(std::env::current_dir().unwrap(), current);
    assert!(
        std::env::var_os("ROLE").is_none(),
        "the process environment is untouched"
    );
}

/// Await every future concurrently on the current task.
async fn futures_join<F: std::future::Future>(
    futures: impl IntoIterator<Item = F>,
) -> Vec<F::Output> {
    let futures: Vec<_> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<Option<F::Output>> = futures.iter().map(|_| None).collect();
    let mut futures: Vec<_> = futures.into_iter().map(Some).collect();
    std::future::poll_fn(|context| {
        let mut pending = false;
        for (index, slot) in futures.iter_mut().enumerate() {
            if let Some(future) = slot {
                match future.as_mut().poll(context) {
                    std::task::Poll::Ready(output) => {
                        outputs[index] = Some(output);
                        *slot = None;
                    }
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    outputs.into_iter().map(Option::unwrap).collect()
}

#[tokio::test]
async fn http_requests_share_no_connections_or_cookies() {
    // Each connection records its requests; responses set a cookie and invite reuse.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let connections: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
    {
        let connections = Arc::clone(&connections);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let index = {
                    let mut all = connections.lock().unwrap();
                    all.push(Vec::new());
                    all.len() - 1
                };
                let connections = Arc::clone(&connections);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let mut request = String::new();
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            if line == "\r\n" {
                                break;
                            }
                            request.push_str(&line);
                        }
                        connections.lock().unwrap()[index].push(request);
                        let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: session=1\r\nConnection: keep-alive\r\n\r\nok";
                        if stream.write_all(response.as_bytes()).is_err() {
                            return;
                        }
                    }
                });
            }
        });
    }
    let engine = Engine::default();
    let directory = tempfile::tempdir().unwrap();
    let source = format!(
        "Log |@{{ HTTP Request |\"GET\"| To |\"{url}\"| }}|\nLog |@{{ HTTP Request |\"GET\"| To |\"{url}\"| }}|"
    );
    for _ in 0..2 {
        let result = engine
            .run_source_async("http.botwork", &source, options(directory.path()))
            .await;
        assert!(result.result.is_ok(), "{:?}", result.result);
    }
    let connections = connections.lock().unwrap();
    assert_eq!(
        connections.len(),
        4,
        "every request opens its own connection"
    );
    for requests in connections.iter() {
        assert_eq!(requests.len(), 1, "no connection is reused: {requests:?}");
        assert!(
            !requests[0].to_ascii_lowercase().contains("cookie:"),
            "no cookie is replayed: {}",
            requests[0]
        );
    }
}
