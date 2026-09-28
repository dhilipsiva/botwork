//! A private blocking receiver for synchronous statements and native workers.
use super::*;
use std::{
    future::Future,
    task::{Context, Poll, Wake, Waker},
};

struct Ready(Mutex<bool>, Condvar);
impl Wake for Ready {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = true;
        self.1.notify_one();
    }
}

impl WorkerHandle {
    pub(crate) fn wait_blocking_retained(self) -> RetainedReport {
        let ready = Arc::new(Ready(Mutex::new(false), Condvar::new()));
        let waker = Waker::from(ready.clone());
        let mut task = Context::from_waker(&waker);
        let mut pending = std::pin::pin!(self.wait_retained());
        loop {
            match pending.as_mut().poll(&mut task) {
                Poll::Ready(report) => return report,
                Poll::Pending => {
                    let state = ready.0.lock().unwrap_or_else(|error| error.into_inner());
                    let mut state = ready
                        .1
                        .wait_while(state, |notified| !*notified)
                        .unwrap_or_else(|error| error.into_inner());
                    *state = false;
                }
            }
        }
    }
}
