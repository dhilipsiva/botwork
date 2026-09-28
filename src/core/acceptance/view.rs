use super::*;
use std::fmt;

/// A status projection, not a complete report schema. Fields are derived only
/// from validated verdicts; HTML never interpolates arbitrary caller strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct StatusView {
    status: &'static str,
    label: &'static str,
    failed: bool,
    complete: bool,
}

impl StatusView {
    pub fn status(&self) -> &'static str {
        self.status
    }
    pub fn label(&self) -> &'static str {
        self.label
    }
    pub fn failed(&self) -> bool {
        self.failed
    }
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn exit_code(&self) -> u8 {
        if self.failed {
            1
        } else {
            0
        }
    }
    pub fn html(self) -> HtmlStatus {
        HtmlStatus(self)
    }
}

impl CaseStatus {
    pub fn view(self) -> StatusView {
        StatusView {
            status: self.as_str(),
            label: self.label(),
            failed: self.failed(),
            complete: self.complete(),
        }
    }
}

impl RunVerdict {
    pub fn view(self) -> StatusView {
        StatusView {
            complete: self.complete(),
            ..self.status().view()
        }
    }
}

/// Constant-vocabulary HTML can stream through the existing admitted output API.
/// Labels/classes are presentation only; data attributes retain the exact verdict.
pub struct HtmlStatus(StatusView);
impl fmt::Display for HtmlStatus {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        write!(output, "<span class=\"status status-{}\" data-outcome=\"{}\" data-failed=\"{}\" data-complete=\"{}\">{}</span>",
            value.status, value.status, value.failed, value.complete, value.label)
    }
}
