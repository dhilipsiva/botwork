use super::*;

#[cfg(test)]
mod tests;
use crate::core::{
    ast::{SourceFile, Span},
    diagnostic::{CallFrame, DiagnosticSize},
};
use std::collections::HashMap;

/// Live immutable call-frame and handler-error records, shared across Context snapshots.
#[derive(Clone, Debug)]
pub struct RetainedDiagnosticLimits {
    pub records: usize,
    pub diagnostics: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    pub text_bytes: usize,
    pub source_bytes: usize,
}

impl Default for RetainedDiagnosticLimits {
    fn default() -> Self {
        Self {
            records: 65_536,
            diagnostics: 16_384,
            call_frames: 65_536,
            related_locations: 65_536,
            text_bytes: 32 * 1024 * 1024,
            source_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Default)]
struct Usage {
    counts: [usize; 5],
    source_bytes: usize,
    sources: HashMap<usize, usize>,
}

pub(super) struct RetainedDiagnostics {
    limits: RetainedDiagnosticLimits,
    used: Mutex<Usage>,
}

fn exceeded(resource: &'static str, maximum: usize) -> BWErr {
    BWErr::ResourceLimit {
        resource,
        limit: maximum as u64,
    }
}
fn source_id(source: &Arc<SourceFile>) -> usize {
    Arc::as_ptr(source) as usize
}

impl RetainedDiagnostics {
    pub(super) fn new(limits: RetainedDiagnosticLimits) -> Self {
        Self {
            limits,
            used: Mutex::new(Usage::default()),
        }
    }

    fn check_record(&self) -> Result<(), BWErr> {
        let used = self.used.lock().unwrap_or_else(|error| error.into_inner());
        if used.counts[0] >= self.limits.records {
            return Err(exceeded("retained diagnostic records", self.limits.records));
        }
        Ok(())
    }

    fn reserve(
        self: &Arc<Self>,
        size: DiagnosticSize,
        sources: Vec<Arc<SourceFile>>,
    ) -> Result<DiagnosticReservation, BWErr> {
        let charge = [
            1,
            size.diagnostics,
            size.call_frames,
            size.related_locations,
            size.text_bytes,
        ];
        let limits = &self.limits;
        let resources = [
            ("retained diagnostic records", limits.records),
            ("retained diagnostic nodes", limits.diagnostics),
            ("retained diagnostic call frames", limits.call_frames),
            (
                "retained diagnostic related locations",
                limits.related_locations,
            ),
            ("retained diagnostic text bytes", limits.text_bytes),
        ];
        let mut used = self.used.lock().unwrap_or_else(|error| error.into_inner());
        let mut next = used.counts;
        for (index, (resource, maximum)) in resources.into_iter().enumerate() {
            next[index] = next[index]
                .checked_add(charge[index])
                .filter(|value| *value <= maximum)
                .ok_or_else(|| exceeded(resource, maximum))?;
        }
        let mut source_bytes = used.source_bytes;
        for source in &sources {
            if let Some(references) = used.sources.get(&source_id(source)) {
                references
                    .checked_add(1)
                    .ok_or_else(|| exceeded("retained diagnostic records", limits.records))?;
            } else {
                source_bytes = source_bytes
                    .checked_add(source.name().len())
                    .and_then(|bytes| bytes.checked_add(source.text().len()))
                    .filter(|bytes| *bytes <= limits.source_bytes)
                    .ok_or_else(|| {
                        exceeded("retained diagnostic source bytes", limits.source_bytes)
                    })?;
            }
        }
        let reservation = DiagnosticReservation {
            owner: Arc::clone(self),
            charge,
            sources,
        };
        for source in &reservation.sources {
            *used.sources.entry(source_id(source)).or_default() += 1;
        }
        used.counts = next;
        used.source_bytes = source_bytes;
        Ok(reservation)
    }
}

pub(crate) struct DiagnosticReservation {
    owner: Arc<RetainedDiagnostics>,
    charge: [usize; 5],
    // Keep pointer identities alive until their tracker entries have been removed.
    sources: Vec<Arc<SourceFile>>,
}

impl Drop for DiagnosticReservation {
    fn drop(&mut self) {
        let mut used = self
            .owner
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (count, charge) in used.counts.iter_mut().zip(self.charge) {
            *count -= charge;
        }
        for source in &self.sources {
            let id = source_id(source);
            let references = used
                .sources
                .get_mut(&id)
                .expect("reserved diagnostic source");
            *references -= 1;
            if *references == 0 {
                used.sources.remove(&id);
                used.source_bytes -= source.name().len() + source.text().len();
            }
        }
    }
}

pub(crate) struct StoredDiagnostic {
    pub(crate) value: Diagnostic,
    // Release owned metadata before its accounting and source-identity references.
    pub(crate) _reservation: Option<DiagnosticReservation>,
}
impl StoredDiagnostic {
    pub(crate) fn into_diagnostic(value: Arc<Self>) -> Diagnostic {
        match Arc::try_unwrap(value) {
            Ok(value) => value.value,
            Err(value) => value.value.clone(),
        }
    }
}

pub(crate) struct StoredCallFrame {
    pub(crate) frame: CallFrame,
    pub(crate) _reservation: Option<DiagnosticReservation>,
}

impl RunBudget {
    pub(crate) fn reserve_diagnostic(
        &self,
        diagnostic: &Diagnostic,
    ) -> DiagnosticResult<DiagnosticReservation> {
        self.checkpoint()?;
        self.0
            .retained_diagnostics
            .check_record()
            .map_err(|error| self.stop(error))?;
        let (size, sources) = self
            .0
            .limits
            .diagnostics
            .retained_size(diagnostic)
            .map_err(|error| self.stop(error))?;
        self.0
            .retained_diagnostics
            .reserve(size, sources)
            .map_err(|error| self.stop(error))
    }

    pub(crate) fn reserve_call_frame(
        &self,
        signature: &str,
        call_site: &Span,
        definition_site: Option<&Span>,
    ) -> DiagnosticResult<DiagnosticReservation> {
        self.checkpoint()?;
        self.0
            .retained_diagnostics
            .check_record()
            .map_err(|error| self.stop(error))?;
        let mut sources = vec![Arc::clone(call_site.source())];
        if let Some(definition) = definition_site {
            if !Arc::ptr_eq(call_site.source(), definition.source()) {
                sources.push(Arc::clone(definition.source()));
            }
        }
        self.0
            .retained_diagnostics
            .reserve(
                DiagnosticSize {
                    call_frames: 1,
                    text_bytes: signature.len(),
                    ..DiagnosticSize::default()
                },
                sources,
            )
            .map_err(|error| self.stop(error))
    }
}
