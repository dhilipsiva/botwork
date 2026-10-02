//! Failure artifacts: what a session showed when a statement in it failed, or
//! when the run ended with it open, saved where `failure_artifacts` says and
//! recorded as the run's artifacts.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) struct Artifacts {
    /// Absolute, and existing.
    directory: PathBuf,
    recorder: Option<crate::core::report::Recorder>,
    taken: AtomicUsize,
    /// The log of the driver Botwork started for the session, if it did.
    driver_log: Option<PathBuf>,
    log_recorded: AtomicBool,
}

impl Artifacts {
    pub(super) fn new(
        directory: PathBuf,
        recorder: Option<crate::core::report::Recorder>,
        driver_log: Option<PathBuf>,
    ) -> Self {
        Self {
            directory,
            recorder,
            taken: AtomicUsize::new(0),
            driver_log,
            log_recorded: AtomicBool::new(false),
        }
    }

    fn record(&self, kind: &str, path: &Path) -> String {
        let path = crate::core::paths::canonicalize(path)
            .unwrap_or_else(|_| path.to_owned())
            .display()
            .to_string();
        if let Some(recorder) = &self.recorder {
            recorder.artifact(kind, &path);
        }
        path
    }

    /// Record the driver's log, once, when a failure needs it.
    pub(super) fn driver_log(&self) -> Option<String> {
        let log = self.driver_log.as_ref()?;
        if self.log_recorded.swap(true, Ordering::SeqCst) {
            return Some(log.display().to_string());
        }
        Some(self.record("driver-log", log))
    }

    /// Remove the driver's log of a session that closed without a failure.
    pub(super) fn discard_log(&self) {
        if let (Some(log), false) = (&self.driver_log, self.log_recorded.load(Ordering::SeqCst)) {
            let _ = std::fs::remove_file(log);
        }
    }

    /// The files of the next capture of `session`.
    fn names(&self, session: &str) -> (PathBuf, PathBuf) {
        let number = self.taken.fetch_add(1, Ordering::SeqCst) + 1;
        let short: String = session
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect();
        let stem = format!("session-{short}-{number}");
        (
            self.directory.join(format!("{stem}.png")),
            self.directory.join(format!("{stem}.html")),
        )
    }

    /// Save the screenshot and page source the driver gives, as far as it
    /// answers, and return the files saved.
    fn save(&self, session: &str, screenshot: Option<Value>, source: Option<Value>) -> Vec<String> {
        let (png, html) = self.names(session);
        let mut saved = Vec::new();
        if let Some(bytes) = screenshot
            .as_ref()
            .and_then(Value::as_str)
            .and_then(|encoded| values::base64(encoded).ok())
        {
            if std::fs::write(&png, bytes).is_ok() {
                saved.push(self.record("failure-screenshot", &png));
            }
        }
        if let Some(text) = source.as_ref().and_then(Value::as_str) {
            if std::fs::write(&html, text).is_ok() {
                saved.push(self.record("failure-source", &html));
            }
        }
        saved.extend(self.driver_log());
        saved
    }

    /// Capture `session` while the run goes on.
    pub(super) async fn capture(
        &self,
        endpoint: &Endpoint,
        session: &str,
        timeout: Duration,
    ) -> Vec<String> {
        let base = format!("/session/{}", segment(session));
        let screenshot = endpoint
            .command(Method::GET, &format!("{base}/screenshot"), None, timeout)
            .await
            .ok();
        let source = endpoint
            .command(Method::GET, &format!("{base}/source"), None, timeout)
            .await
            .ok();
        self.save(session, screenshot, source)
    }

    /// Capture `session` as the run ends, without a runtime.
    pub(super) fn capture_blocking(
        &self,
        endpoint: &Endpoint,
        session: &str,
        timeout: Duration,
    ) -> Vec<String> {
        let base = format!("/session/{}", segment(session));
        let screenshot = endpoint.get_blocking(&format!("{base}/screenshot"), timeout);
        let source = endpoint.get_blocking(&format!("{base}/source"), timeout);
        self.save(session, screenshot, source)
    }
}

/// `error`, naming the failure artifacts saved for it.
pub(super) fn naming(error: Diagnostic, saved: &[String]) -> Diagnostic {
    if saved.is_empty() {
        return error;
    }
    let note = format!("; failure artifacts: {}", saved.join(", "));
    match &*error.error {
        BWErr::NativeError(message) => {
            Diagnostic::new(BWErr::NativeError(format!("{message}{note}")))
        }
        BWErr::ConditionNotMet {
            reason,
            attempts,
            history,
        } => Diagnostic::new(BWErr::ConditionNotMet {
            reason: format!("{reason}{note}"),
            attempts: attempts.clone(),
            history: history.clone(),
        }),
        _ => error,
    }
}
