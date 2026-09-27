use super::*;

#[cfg(test)]
mod tests;
use crate::core::{
    ast::{SourceFile, Span},
    diagnostic::{CallFrame, DiagnosticSize},
};
use std::collections::HashMap;

/// Live call, handler, and admitted outgoing diagnostic records shared across Context snapshots.
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
        self.replace(size, sources, &mut Vec::new())
    }

    fn replace(
        self: &Arc<Self>,
        size: DiagnosticSize,
        sources: Vec<Arc<SourceFile>>,
        previous: &mut Vec<DiagnosticReservation>,
    ) -> Result<DiagnosticReservation, BWErr> {
        self.replace_with_source(size, sources, previous, 0)
    }

    fn replace_with_source(
        self: &Arc<Self>,
        size: DiagnosticSize,
        sources: Vec<Arc<SourceFile>>,
        previous: &mut Vec<DiagnosticReservation>,
        pending_source_bytes: usize,
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
        let mut source_bytes = used.source_bytes;
        let mut changes: HashMap<usize, (&Arc<SourceFile>, usize, usize)> = HashMap::new();
        for reservation in previous
            .iter()
            .filter(|value| Arc::ptr_eq(&value.owner, self))
        {
            source_bytes -= reservation.pending_source_bytes;
            for (value, old) in next.iter_mut().zip(reservation.charge) {
                *value -= old;
            }
            for source in &reservation.sources {
                changes.entry(source_id(source)).or_insert((source, 0, 0)).1 += 1;
            }
        }
        for (index, (resource, maximum)) in resources.into_iter().enumerate() {
            next[index] = next[index]
                .checked_add(charge[index])
                .filter(|value| *value <= maximum)
                .ok_or_else(|| exceeded(resource, maximum))?;
        }
        for source in &sources {
            changes.entry(source_id(source)).or_insert((source, 0, 0)).2 += 1;
        }
        let mut references = Vec::with_capacity(changes.len());
        // Release identities no longer owned before admitting replacement owners.
        for (&id, &(source, removed, added)) in &changes {
            let before = used.sources.get(&id).copied().unwrap_or_default();
            let after = (before - removed)
                .checked_add(added)
                .ok_or_else(|| exceeded("retained diagnostic records", limits.records))?;
            if before != 0 && after == 0 {
                source_bytes -= source.name().len() + source.text().len();
            }
            references.push((id, before, after));
        }
        for &(id, before, after) in &references {
            if before == 0 && after != 0 {
                let source = changes[&id].0;
                source_bytes = source_bytes
                    .checked_add(source.name().len())
                    .and_then(|bytes| bytes.checked_add(source.text().len()))
                    .filter(|bytes| *bytes <= limits.source_bytes)
                    .ok_or_else(|| {
                        exceeded("retained diagnostic source bytes", limits.source_bytes)
                    })?;
            }
        }
        source_bytes = source_bytes
            .checked_add(pending_source_bytes)
            .filter(|bytes| *bytes <= limits.source_bytes)
            .ok_or_else(|| exceeded("retained diagnostic source bytes", limits.source_bytes))?;
        drop(changes);
        let reservation = DiagnosticReservation {
            owner: Arc::clone(self),
            charge,
            sources,
            pending_source_bytes,
        };
        for (id, _, after) in references {
            if after == 0 {
                used.sources.remove(&id);
            } else {
                used.sources.insert(id, after);
            }
        }
        used.counts = next;
        used.source_bytes = source_bytes;
        for old in previous
            .iter_mut()
            .filter(|value| Arc::ptr_eq(&value.owner, self))
        {
            old.charge = [0; 5];
            old.sources.clear();
            old.pending_source_bytes = 0;
        }
        drop(used);
        previous.clear();
        Ok(reservation)
    }
}

pub(crate) struct DiagnosticReservation {
    owner: Arc<RetainedDiagnostics>,
    charge: [usize; 5],
    // Keep pointer identities alive until their tracker entries have been removed.
    sources: Vec<Arc<SourceFile>>,
    // Bytes reserved before a new source allocation has an identity to track.
    pending_source_bytes: usize,
}

impl DiagnosticReservation {
    pub(crate) fn publish_source(&mut self, source: &Arc<SourceFile>) {
        assert_eq!(
            self.pending_source_bytes,
            source.name().len() + source.text().len()
        );
        let mut used = self
            .owner
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // This allocation has not escaped construction; it cannot already be owned.
        let id = source_id(source);
        assert!(!used.sources.contains_key(&id));
        self.sources.push(Arc::clone(source));
        used.sources.insert(id, 1);
        self.pending_source_bytes = 0;
    }
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
        used.source_bytes -= self.pending_source_bytes;
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

/// Internal ownership carrier. Raw host diagnostics enter without runtime leases;
/// retained, copied, and mutation-admitted records keep leases through evaluation and disposal.
pub(crate) struct RuntimeDiagnostic {
    value: Box<crate::core::diagnostic::OwnedDiagnostic>,
    reservations: Vec<DiagnosticReservation>,
}

impl std::fmt::Debug for RuntimeDiagnostic {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.value.as_ref().as_ref(), output)
    }
}
impl std::fmt::Display for RuntimeDiagnostic {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.value.as_ref().as_ref(), output)
    }
}
impl std::ops::Deref for RuntimeDiagnostic {
    type Target = Diagnostic;
    fn deref(&self) -> &Diagnostic {
        self.value.as_ref().as_ref()
    }
}
impl From<BWErr> for RuntimeDiagnostic {
    fn from(value: BWErr) -> Self {
        Diagnostic::new(value).into()
    }
}
impl From<Diagnostic> for RuntimeDiagnostic {
    fn from(value: Diagnostic) -> Self {
        Self {
            value: Box::new(crate::core::diagnostic::OwnedDiagnostic::new(value)),
            reservations: vec![],
        }
    }
}
impl From<StoredDiagnostic> for RuntimeDiagnostic {
    fn from(value: StoredDiagnostic) -> Self {
        Self {
            value: Box::new(crate::core::diagnostic::OwnedDiagnostic::new(value.value)),
            reservations: value._reservation.into_iter().collect(),
        }
    }
}
impl RuntimeDiagnostic {
    pub(crate) fn constructed(
        value: Diagnostic,
        reservation: Option<DiagnosticReservation>,
    ) -> Self {
        Self::from(StoredDiagnostic {
            value,
            _reservation: reservation,
        })
    }
    pub(crate) fn into_diagnostic(self) -> Diagnostic {
        (*self.value).into_inner()
    }

    pub(crate) fn map(mut self, transform: impl FnOnce(Diagnostic) -> Diagnostic) -> Self {
        self.value.map(transform);
        if self.is_emergency() {
            // Full metadata has already been disposed; emergency evidence owns
            // no source trees and uses the independent fixed summary allowance.
            self.reservations.clear();
        }
        self
    }

    pub(crate) fn with_context<'a>(
        mut self,
        context: Option<(&Span, bool)>,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
        budget: Option<&RunBudget>,
    ) -> Self {
        if self.is_emergency() {
            return self;
        }
        let stopped = budget.and_then(|budget| budget.checkpoint().err());
        let preserve_control = stopped.as_ref().is_some_and(|stop| {
            stop.code() == self.code()
                && matches!(
                    stop.code(),
                    DiagnosticCode::Cancelled | DiagnosticCode::Timeout
                )
        });
        if let Err(violation) = self.admit_mutation(budget, context, frames.clone(), None, None) {
            if let Some(budget) = budget {
                budget.stop(violation.clone());
            }
            let error = self.reject_context(violation, context, frames.len());
            return if preserve_control {
                error.preserve_control()
            } else {
                error
            };
        }
        self.map(|error| error.capture_context(context, frames))
    }

    pub(crate) fn with_related(
        mut self,
        message: &str,
        span: &Span,
        budget: Option<&RunBudget>,
    ) -> Self {
        if !self.is_emergency() {
            if let Err(violation) = self.admit_mutation(
                budget,
                None,
                std::iter::empty(),
                Some((message, span)),
                None,
            ) {
                return self
                    .reject_mutation(violation, budget, None, 0)
                    .map(|error| error.with_related(message, span));
            }
        }
        self.map(|value| value.with_related(message, span))
    }

    pub(crate) fn while_handling(self, original: Self, budget: Option<&RunBudget>) -> Self {
        self.while_handling_in(original, budget, None, std::iter::empty())
    }

    pub(crate) fn while_handling_in<'a>(
        mut self,
        mut original: Self,
        budget: Option<&RunBudget>,
        context: Option<(&Span, bool)>,
        frames: impl ExactSizeIterator<Item = &'a CallFrame> + DoubleEndedIterator + Clone,
    ) -> Self {
        if Arc::ptr_eq(&self.error, &original.error) {
            return self;
        }
        if self.is_emergency() {
            drop(original);
            return self.map(Diagnostic::omit_handled_cause);
        }
        // Transfer leases before measuring the combined tree so replacing them
        // credits both inputs atomically without double-reserving moved payloads.
        self.reservations.append(&mut original.reservations);
        if let Err(violation) =
            self.admit_mutation(budget, context, frames.clone(), None, Some(&original))
        {
            drop(original);
            return self
                .reject_mutation(violation, budget, context, frames.len())
                .map(Diagnostic::omit_handled_cause);
        }
        self.map(|error| {
            error
                .while_handling((*original.value).into_inner())
                .capture_context(context, frames)
        })
    }

    fn admit_mutation<'a>(
        &mut self,
        budget: Option<&RunBudget>,
        context: Option<(&Span, bool)>,
        frames: impl ExactSizeIterator<Item = &'a CallFrame>,
        related: Option<(&str, &Span)>,
        cause: Option<&Diagnostic>,
    ) -> Result<(), BWErr> {
        // Observe a stop before any new quota can latch, but still allow bounded
        // diagnostic evidence to be attached during cancellation unwinding.
        if let Some(budget) = budget {
            let _ = budget.checkpoint();
        }
        let defaults = DiagnosticLimits::default();
        let limits = budget.map_or(&defaults, |budget| &budget.limits().diagnostics);
        let (size, sources) =
            limits.retained_runtime_size(self, frames, context, related, cause)?;
        if let Some(budget) = budget {
            let reservation =
                budget
                    .0
                    .retained_diagnostics
                    .replace(size, sources, &mut self.reservations)?;
            self.reservations.push(reservation);
        }
        Ok(())
    }

    fn reject_mutation(
        self,
        violation: BWErr,
        budget: Option<&RunBudget>,
        context: Option<(&Span, bool)>,
        pending_frames: usize,
    ) -> Self {
        let stopped = budget.map(|budget| budget.stop(violation.clone()));
        if matches!(
            self.code(),
            DiagnosticCode::Cancelled | DiagnosticCode::Timeout
        ) && stopped
            .as_ref()
            .is_none_or(|stop| stop.code() == self.code())
        {
            // Keep an observed control failure primary even when its added
            // evidence exceeds a quota. The original tree is disposed first.
            self.reject_context(violation, context, pending_frames)
                .preserve_control()
        } else {
            self.reject_context(
                stopped.map_or(violation, Diagnostic::into_error),
                context,
                pending_frames,
            )
        }
    }

    fn preserve_control(self) -> Self {
        self.map(|mut error| {
            let mut original = error.causes.pop().expect("bounded original");
            original.causes.push(error);
            original
        })
    }

    pub(crate) fn reject(self, violation: BWErr) -> Self {
        self.reject_context(violation, None, 0)
    }

    fn reject_context(
        self,
        violation: BWErr,
        context: Option<(&Span, bool)>,
        pending_frames: usize,
    ) -> Self {
        // Dispose the old payload before refunding its retained ownership.
        let Self {
            value,
            reservations,
        } = self;
        let error = (*value)
            .into_inner()
            .rejected_context(violation, context, pending_frames);
        drop(reservations);
        error.into()
    }

    pub(crate) fn store(
        mut self,
        budget: Option<&RunBudget>,
    ) -> DiagnosticResult<Arc<StoredDiagnostic>> {
        let reservation = if let Some(budget) = budget {
            budget.checkpoint().and_then(|()| {
                let (size, sources) = budget
                    .limits()
                    .diagnostics
                    .retained_copy_size(&self, None)
                    .map_err(|error| budget.stop(error))?;
                budget
                    .0
                    .retained_diagnostics
                    .replace(size, sources, &mut self.reservations)
                    .map(Some)
                    .map_err(|error| budget.stop(error))
            })
        } else {
            Ok(None)
        };
        match reservation {
            Ok(reservation) => Ok(Arc::new(StoredDiagnostic {
                value: (*self.value).into_inner(),
                _reservation: reservation,
            })),
            Err(error) => Err(self.reject(error.into_error()).into_diagnostic()),
        }
    }
}
impl StoredDiagnostic {
    /// Keep the copied tree's reservation alive until its explicit handoff.
    pub(crate) fn copy(
        &self,
        budget: Option<&RunBudget>,
        related: Option<(&str, &Span)>,
    ) -> DiagnosticResult<Self> {
        let reservation = match budget {
            Some(budget) => Some(budget.reserve_diagnostic_copy(&self.value, related)?),
            None => {
                DiagnosticLimits::default().retained_copy_size(&self.value, related)?;
                None
            }
        };
        let mut value = self.value.clone();
        if let Some((message, span)) = related {
            value = value.with_related(message, span);
        }
        Ok(Self {
            value,
            _reservation: reservation,
        })
    }

    pub(crate) fn take(value: Arc<Self>, budget: Option<&RunBudget>) -> DiagnosticResult<Self> {
        match Arc::try_unwrap(value) {
            Ok(value) => Ok(value),
            Err(value) => value.copy(budget, None),
        }
    }
}

pub(crate) struct StoredCallFrame {
    pub(crate) frame: CallFrame,
    pub(crate) _reservation: Option<DiagnosticReservation>,
}

impl RunBudget {
    pub(crate) fn reserve_diagnostic_source_construction(
        &self,
        size: DiagnosticSize,
        sources: Vec<Arc<SourceFile>>,
        pending_source_bytes: usize,
    ) -> Result<DiagnosticReservation, BWErr> {
        let _ = self.checkpoint();
        self.0.retained_diagnostics.replace_with_source(
            size,
            sources,
            &mut Vec::new(),
            pending_source_bytes,
        )
    }

    pub(crate) fn reserve_diagnostic_construction(
        &self,
        size: DiagnosticSize,
        sources: Vec<Arc<SourceFile>>,
        previous: Option<DiagnosticReservation>,
    ) -> Result<DiagnosticReservation, BWErr> {
        // Observe a newly requested stop before construction failure can latch.
        let _ = self.checkpoint();
        self.0
            .retained_diagnostics
            .replace(size, sources, &mut previous.into_iter().collect())
    }

    fn reserve_diagnostic_copy(
        &self,
        diagnostic: &Diagnostic,
        related: Option<(&str, &Span)>,
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
            .retained_copy_size(diagnostic, related)
            .map_err(|error| self.stop(error))?;
        self.0
            .retained_diagnostics
            .reserve(size, sources)
            .map_err(|error| self.stop(error))
    }

    #[cfg(test)]
    pub(crate) fn reserve_diagnostic(
        &self,
        diagnostic: &Diagnostic,
    ) -> DiagnosticResult<DiagnosticReservation> {
        self.reserve_diagnostic_copy(diagnostic, None)
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
