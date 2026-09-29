//! Bounded, isolated delivery to report listeners. Producers never wait for a
//! listener: items are queued without blocking, a dedicated thread delivers them
//! in queue order, and a listener that fails or falls too far behind is detached
//! with a recorded [`ListenerFailure`] instead of slowing or changing the runs.
use serde::Serialize;
use std::{
    collections::VecDeque,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
mod tests;

/// Receives queued items on the dispatcher's thread. Both methods may block.
pub trait Listener<T>: Send {
    /// Deliver one item; an error detaches the listener.
    fn deliver(&mut self, item: &T) -> Result<(), String>;
    /// Called once when delivery ends, after a failure too, so the listener can
    /// release resources. Its error counts only when delivery had not failed.
    fn finish(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerOptions {
    /// Items queued but not yet delivered; one more detaches the listener.
    pub capacity: usize,
    /// How long `close` waits for queued items and `finish`.
    pub close_timeout: Duration,
}

pub const DEFAULT_LISTENER_CAPACITY: usize = 8192;
pub const DEFAULT_LISTENER_CLOSE_TIMEOUT: Duration = Duration::from_secs(10);

impl Default for ListenerOptions {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_LISTENER_CAPACITY,
            close_timeout: DEFAULT_LISTENER_CLOSE_TIMEOUT,
        }
    }
}

/// Why a listener stopped receiving items. The first failure is kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ListenerFailure {
    /// `deliver` or `finish` returned an error.
    Rejected { message: String },
    /// `deliver` or `finish` panicked.
    Panicked,
    /// `capacity` items were already waiting when another arrived.
    Overflow { capacity: usize },
    /// Delivery and `finish` did not complete within the close timeout.
    Timeout { pending: usize },
}

impl fmt::Display for ListenerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { message } => write!(f, "listener failed: {message}"),
            Self::Panicked => f.write_str("listener panicked"),
            Self::Overflow { capacity } => {
                write!(f, "listener fell {capacity} items behind and was detached")
            }
            Self::Timeout { pending } => write!(
                f,
                "listener did not finish within the close timeout ({pending} items undelivered)"
            ),
        }
    }
}

/// Counts for one listener's stream. `accepted - delivered` items were lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerOutcome {
    pub accepted: u64,
    pub delivered: u64,
    pub failure: Option<ListenerFailure>,
    /// False when the delivery thread was still running at the close deadline,
    /// for example blocked in `deliver`; the host should stop what it waits on.
    pub finished: bool,
}

struct State<T> {
    queue: VecDeque<T>,
    closed: bool,
    finished: bool,
    accepted: u64,
    delivered: u64,
    failure: Option<ListenerFailure>,
}

struct Shared<T> {
    capacity: usize,
    state: Mutex<State<T>>,
    /// Signals the delivery thread: an item, a failure, or close.
    ready: Condvar,
    /// Signals `close`: the delivery thread has finished.
    done: Condvar,
}

impl<T> Shared<T> {
    fn state(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Keep the first failure and discard what can no longer be delivered.
    fn fail(state: &mut State<T>, failure: ListenerFailure) {
        state.failure.get_or_insert(failure);
        state.queue.clear();
    }
}

/// Queues items for one listener. Clones share the queue.
pub struct ListenerSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for ListenerSender<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> fmt::Debug for ListenerSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ListenerSender")
    }
}

impl<T> ListenerSender<T> {
    /// Queue `item` without blocking. Returns false when the listener is closed
    /// or detached; an item beyond the capacity detaches it.
    pub fn send(&self, item: T) -> bool {
        let mut state = self.shared.state();
        if state.closed || state.failure.is_some() {
            return false;
        }
        if state.queue.len() >= self.shared.capacity {
            Shared::fail(
                &mut state,
                ListenerFailure::Overflow {
                    capacity: self.shared.capacity,
                },
            );
            self.shared.ready.notify_one();
            return false;
        }
        state.queue.push_back(item);
        state.accepted += 1;
        self.shared.ready.notify_one();
        true
    }
}

/// Owns one listener's delivery thread.
pub struct Dispatcher<T> {
    shared: Arc<Shared<T>>,
    thread: Option<thread::JoinHandle<()>>,
    close_timeout: Duration,
}

impl<T: Send + 'static> Dispatcher<T> {
    /// Start delivering to `listener` on a new thread.
    pub fn spawn(
        listener: impl Listener<T> + 'static,
        options: ListenerOptions,
    ) -> std::io::Result<Self> {
        if options.capacity == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "listener capacity must be at least 1",
            ));
        }
        let shared = Arc::new(Shared {
            capacity: options.capacity,
            state: Mutex::new(State {
                queue: VecDeque::new(),
                closed: false,
                finished: false,
                accepted: 0,
                delivered: 0,
                failure: None,
            }),
            ready: Condvar::new(),
            done: Condvar::new(),
        });
        let thread = thread::Builder::new()
            .name("botwork-listener".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || deliver(listener, &shared)
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
            close_timeout: options.close_timeout,
        })
    }
}

impl<T> Dispatcher<T> {
    pub fn sender(&self) -> ListenerSender<T> {
        ListenerSender {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Stop accepting items, then wait up to the close timeout for queued items
    /// and `finish`. A listener still running afterwards is detached.
    pub fn close(mut self) -> ListenerOutcome {
        let deadline = Instant::now() + self.close_timeout;
        let mut state = self.shared.state();
        state.closed = true;
        self.shared.ready.notify_one();
        while !state.finished {
            let now = Instant::now();
            if now >= deadline {
                let pending = state.queue.len();
                Shared::fail(&mut state, ListenerFailure::Timeout { pending });
                break;
            }
            state = self
                .shared
                .done
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        let finished = state.finished;
        let outcome = ListenerOutcome {
            accepted: state.accepted,
            delivered: state.delivered,
            failure: state.failure.clone(),
            finished,
        };
        drop(state);
        if let Some(thread) = self.thread.take().filter(|_| finished) {
            let _ = thread.join();
        }
        outcome
    }
}

impl<T> Drop for Dispatcher<T> {
    fn drop(&mut self) {
        // Without `close`, stop accepting items and let the thread end on its own.
        self.shared.state().closed = true;
        self.shared.ready.notify_one();
    }
}

fn deliver<T>(mut listener: impl Listener<T>, shared: &Shared<T>) {
    let mut state = shared.state();
    loop {
        if state.failure.is_some() {
            break;
        }
        let Some(item) = state.queue.pop_front() else {
            if state.closed {
                break;
            }
            state = shared
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
            continue;
        };
        drop(state);
        let result = catch_unwind(AssertUnwindSafe(|| listener.deliver(&item)));
        drop(item);
        state = shared.state();
        match result {
            Ok(Ok(())) => state.delivered += 1,
            Ok(Err(message)) => Shared::fail(&mut state, ListenerFailure::Rejected { message }),
            Err(_) => Shared::fail(&mut state, ListenerFailure::Panicked),
        }
    }
    drop(state);
    let result = catch_unwind(AssertUnwindSafe(|| listener.finish()));
    let mut state = shared.state();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(message)) => Shared::fail(&mut state, ListenerFailure::Rejected { message }),
        Err(_) => Shared::fail(&mut state, ListenerFailure::Panicked),
    }
    state.finished = true;
    shared.done.notify_all();
}
