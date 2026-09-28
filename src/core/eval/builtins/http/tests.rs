use super::*;

#[tokio::test]
async fn started_dns_retains_capacity_after_owner_drop_until_worker_result_is_destroyed() {
    let context = Context::default();
    let global = Global::new(&context, GLOBAL_BYTES).unwrap();
    assert!(Global::new(&context, 1).is_err());
    let slots = Arc::new(Semaphore::new(1));
    let retention = Arc::new(Retention {
        _slot: slots.clone().try_acquire_owned().unwrap(),
        _workspace: None,
        _workspace_global: global,
        _output_global: Global(0),
    });
    let weak = Arc::downgrade(&retention);
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let pending = transport::PendingDns(tokio::task::spawn_blocking(move || {
        started.send(()).unwrap();
        wait.recv().unwrap();
        (Ok(Vec::new()), retention)
    }));
    ready.await.unwrap();
    drop(pending);
    assert!(weak.upgrade().is_some());
    assert_eq!(slots.available_permits(), 0);
    assert!(Global::new(&context, 1).is_err());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(slots.available_permits(), 1);
    assert!(Global::new(&context, GLOBAL_BYTES).is_ok());
}

#[test]
fn planned_output_dominates_binary_and_text_header_body_shapes() {
    for binary in [false, true] {
        let context = Context::default();
        let plan = config::Plan::new(
            &context,
            binary,
            &Literal::String("GET".into()),
            &Literal::String("http://localhost/".into()),
            None,
        )
        .unwrap();
        let size = output::planned(&context, &plan).unwrap();
        let mut headers = hyper::HeaderMap::new();
        for i in 0..128 {
            headers.append(
                hyper::header::HeaderName::from_bytes(format!("x-{i:03}").as_bytes()).unwrap(),
                hyper::header::HeaderValue::from_str(&"a".repeat(123)).unwrap(),
            );
        }
        let response = transport::Response {
            status: 200,
            headers,
            body: vec![b'a'; plan.max_body],
            url: "a".repeat(MAX_URL),
            redirects: 10,
            attempts: 16,
        };
        let result = output::build(&context, binary, response, size, None).unwrap();
        let actual = context.limits().values.check(&result).unwrap();
        assert!(
            actual.nodes <= size.nodes
                && actual.payload_bytes <= size.payload_bytes
                && actual.depth <= size.depth
        );
    }
}
