use super::*;
use std::sync::mpsc;

/// Records delivered items; optionally blocks until released or fails on one item.
struct Probe {
    delivered: Arc<Mutex<Vec<u32>>>,
    finished: Arc<Mutex<u32>>,
    gate: Option<mpsc::Receiver<()>>,
    fail_on: Option<u32>,
    panic_on: Option<u32>,
    finish_error: bool,
}

type Delivered = Arc<Mutex<Vec<u32>>>;
type Finished = Arc<Mutex<u32>>;

impl Probe {
    fn new() -> (Self, Delivered, Finished) {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(Mutex::new(0));
        (
            Self {
                delivered: Arc::clone(&delivered),
                finished: Arc::clone(&finished),
                gate: None,
                fail_on: None,
                panic_on: None,
                finish_error: false,
            },
            delivered,
            finished,
        )
    }
}

impl Listener<u32> for Probe {
    fn deliver(&mut self, item: &u32) -> Result<(), String> {
        if let Some(gate) = &self.gate {
            let _ = gate.recv();
        }
        if self.panic_on == Some(*item) {
            // Unwind without the panic hook: printing a backtrace can take
            // over a minute in an unoptimized Windows build.
            std::panic::resume_unwind(Box::new("probe panic"));
        }
        if self.fail_on == Some(*item) {
            return Err(format!("rejected {item}"));
        }
        self.delivered.lock().unwrap().push(*item);
        Ok(())
    }
    fn finish(&mut self) -> Result<(), String> {
        *self.finished.lock().unwrap() += 1;
        if self.finish_error {
            Err("finish failed".into())
        } else {
            Ok(())
        }
    }
}

fn options(capacity: usize, close_timeout_ms: u64) -> ListenerOptions {
    ListenerOptions {
        capacity,
        close_timeout: Duration::from_millis(close_timeout_ms),
    }
}

#[test]
fn items_from_many_senders_are_delivered_once_in_queue_order() {
    let (probe, delivered, finished) = Probe::new();
    let dispatcher = Dispatcher::spawn(probe, options(4096, 5000)).unwrap();
    let late = dispatcher.sender();
    let senders: Vec<_> = (0..4).map(|_| dispatcher.sender()).collect();
    let threads: Vec<_> = senders
        .into_iter()
        .enumerate()
        .map(|(thread, sender)| {
            std::thread::spawn(move || {
                for item in 0..250 {
                    assert!(sender.send(thread as u32 * 1000 + item));
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let outcome = dispatcher.close();
    assert_eq!(
        outcome,
        ListenerOutcome {
            accepted: 1000,
            delivered: 1000,
            failure: None,
            finished: true,
        }
    );
    let delivered = delivered.lock().unwrap();
    for thread in 0..4 {
        let own: Vec<_> = delivered
            .iter()
            .filter(|item| **item / 1000 == thread)
            .copied()
            .collect();
        assert_eq!(
            own,
            (0..250)
                .map(|item| thread * 1000 + item)
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(*finished.lock().unwrap(), 1);
    assert!(!late.send(0), "a closed listener refuses items");
}

#[test]
fn a_rejected_item_detaches_the_listener_and_later_items_are_refused() {
    let (mut probe, delivered, finished) = Probe::new();
    probe.fail_on = Some(2);
    let dispatcher = Dispatcher::spawn(probe, options(16, 5000)).unwrap();
    let sender = dispatcher.sender();
    for item in 0..3 {
        assert!(sender.send(item));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while dispatcher.shared.state().failure.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!sender.send(3), "a detached listener refuses items");
    let outcome = dispatcher.close();
    assert_eq!(
        outcome.failure,
        Some(ListenerFailure::Rejected {
            message: "rejected 2".into()
        })
    );
    assert_eq!(&delivered.lock().unwrap()[..2], [0, 1]);
    assert!(!delivered.lock().unwrap().contains(&2));
    assert_eq!(
        *finished.lock().unwrap(),
        1,
        "finish still releases resources"
    );
}

#[test]
fn panics_and_finish_errors_are_failures() {
    let (mut probe, _, _) = Probe::new();
    probe.panic_on = Some(0);
    // The close timeout only guards against a hang.
    let dispatcher = Dispatcher::spawn(probe, options(16, 60_000)).unwrap();
    assert!(dispatcher.sender().send(0));
    assert_eq!(dispatcher.close().failure, Some(ListenerFailure::Panicked));

    let (mut probe, _, _) = Probe::new();
    probe.finish_error = true;
    let dispatcher = Dispatcher::spawn(probe, options(16, 5000)).unwrap();
    assert!(dispatcher.sender().send(1));
    let outcome = dispatcher.close();
    assert_eq!(outcome.delivered, 1);
    assert_eq!(
        outcome.failure,
        Some(ListenerFailure::Rejected {
            message: "finish failed".into()
        })
    );
}

#[test]
fn a_listener_behind_by_the_capacity_is_detached_without_blocking_senders() {
    let (mut probe, delivered, finished) = Probe::new();
    let (release, gate) = mpsc::channel();
    probe.gate = Some(gate);
    let dispatcher = Dispatcher::spawn(probe, options(2, 5000)).unwrap();
    let sender = dispatcher.sender();
    // Item 0 is taken by the blocked listener; items 1 and 2 fill the queue.
    assert!(sender.send(0));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dispatcher.shared.state().queue.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(sender.send(1) && sender.send(2));
    let start = Instant::now();
    assert!(!sender.send(3), "the item beyond the capacity is refused");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "senders never wait"
    );
    assert!(!sender.send(4));
    drop(release);
    let outcome = dispatcher.close();
    assert_eq!(
        outcome.failure,
        Some(ListenerFailure::Overflow { capacity: 2 })
    );
    assert_eq!((outcome.accepted, outcome.delivered), (3, 1));
    assert!(outcome.finished);
    assert_eq!(*delivered.lock().unwrap(), [0]);
    assert_eq!(*finished.lock().unwrap(), 1);
}

#[test]
fn close_detaches_a_listener_that_exceeds_the_timeout() {
    let (mut probe, _, finished) = Probe::new();
    let (release, gate) = mpsc::channel();
    probe.gate = Some(gate);
    let dispatcher = Dispatcher::spawn(probe, options(16, 100)).unwrap();
    let sender = dispatcher.sender();
    for item in 0..3 {
        assert!(sender.send(item));
    }
    let start = Instant::now();
    let outcome = dispatcher.close();
    let waited = start.elapsed();
    assert!(waited >= Duration::from_millis(100) && waited < Duration::from_secs(5));
    assert!(!outcome.finished, "the blocked listener is detached");
    assert!(matches!(
        outcome.failure,
        Some(ListenerFailure::Timeout { pending }) if pending <= 2
    ));
    assert!(!sender.send(9), "a closed listener refuses items");
    drop(release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while *finished.lock().unwrap() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        *finished.lock().unwrap(),
        1,
        "a detached listener still finishes"
    );
}

#[test]
fn zero_capacity_is_rejected_and_failures_render_their_cause() {
    let (probe, _, _) = Probe::new();
    assert!(Dispatcher::spawn(probe, options(0, 10)).is_err());
    assert_eq!(
        ListenerFailure::Overflow { capacity: 8 }.to_string(),
        "listener fell 8 items behind and was detached"
    );
    assert_eq!(
        ListenerFailure::Timeout { pending: 3 }.to_string(),
        "listener did not finish within the close timeout (3 items undelivered)"
    );
}
