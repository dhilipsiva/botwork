//! Shared logical ownership across operation invocations, including detached workers.
use super::*;
use crate::core::{diagnostic::DiagnosticSize, value_limits::ValueSize};
use std::sync::Mutex;

#[cfg(test)]
mod tests;

/// Aggregate allowances shared by one or more operations. Zero and raised limits are valid.
#[derive(Clone, Debug)]
pub struct OperationOwnershipLimits {
    pub invocations: usize,
    pub values: usize,
    pub nodes: usize,
    pub payload_bytes: usize,
    pub diagnostics: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    pub text_bytes: usize,
    pub source_bytes: usize,
}

impl Default for OperationOwnershipLimits {
    fn default() -> Self {
        Self {
            invocations: 1024,
            values: 65_536,
            nodes: 262_144,
            payload_bytes: 32 * 1024 * 1024,
            diagnostics: 16_384,
            call_frames: 65_536,
            related_locations: 65_536,
            text_bytes: 32 * 1024 * 1024,
            source_bytes: 32 * 1024 * 1024,
        }
    }
}

/// Current logical charges. Sources are deduplicated within each diagnostic tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationUsage {
    pub invocations: usize,
    pub values: usize,
    pub nodes: usize,
    pub payload_bytes: usize,
    pub diagnostics: usize,
    pub call_frames: usize,
    pub related_locations: usize,
    pub text_bytes: usize,
    pub source_bytes: usize,
}

type Counts = [usize; 9];

#[derive(Debug)]
struct State {
    limits: OperationOwnershipLimits,
    used: Mutex<Counts>,
}

/// Clone to share admission across operations. Construct another budget for isolation.
#[derive(Clone, Debug)]
pub struct OperationBudget(Arc<State>);

impl Default for OperationBudget {
    fn default() -> Self {
        Self::new(OperationOwnershipLimits::default())
    }
}

impl OperationBudget {
    pub fn new(limits: OperationOwnershipLimits) -> Self {
        Self(Arc::new(State {
            limits,
            used: Mutex::new([0; 9]),
        }))
    }

    pub fn limits(&self) -> &OperationOwnershipLimits {
        &self.0.limits
    }

    pub fn usage(&self) -> OperationUsage {
        let [invocations, values, nodes, payload_bytes, diagnostics, call_frames, related_locations, text_bytes, source_bytes] =
            *self
                .0
                .used
                .lock()
                .unwrap_or_else(|error| error.into_inner());
        OperationUsage {
            invocations,
            values,
            nodes,
            payload_bytes,
            diagnostics,
            call_frames,
            related_locations,
            text_bytes,
            source_bytes,
        }
    }

    fn resources(&self) -> [(&'static str, usize); 9] {
        let limits = self.limits();
        [
            ("operation invocations", limits.invocations),
            ("operation values", limits.values),
            ("operation value nodes", limits.nodes),
            ("operation value payload bytes", limits.payload_bytes),
            ("operation diagnostic nodes", limits.diagnostics),
            ("operation diagnostic call frames", limits.call_frames),
            (
                "operation diagnostic related locations",
                limits.related_locations,
            ),
            ("operation diagnostic text bytes", limits.text_bytes),
            ("operation diagnostic source bytes", limits.source_bytes),
        ]
    }

    fn replace(&self, previous: Counts, charge: Counts) -> Result<(), BWErr> {
        let mut used = self
            .0
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut next = *used;
        for (index, (resource, limit)) in self.resources().into_iter().enumerate() {
            next[index] = (next[index] - previous[index])
                .checked_add(charge[index])
                .filter(|value| *value <= limit)
                .ok_or(BWErr::ResourceLimit {
                    resource,
                    limit: limit as u64,
                })?;
        }
        *used = next;
        Ok(())
    }

    pub(super) fn reserve(&self, charge: Counts) -> Result<Reservation, BWErr> {
        self.replace([0; 9], charge)?;
        Ok(Reservation {
            budget: self.clone(),
            charge,
        })
    }

    pub(super) fn arguments(
        &self,
        values: &[Literal],
        limits: &ValueLimits,
        mut validate: impl FnMut(usize, &Literal) -> Result<(), AdmissionFailure>,
    ) -> Result<Reservation, AdmissionFailure> {
        let mut charge = [1, values.len(), 0, 0, 0, 0, 0, 0, 0];
        for (index, value) in values.iter().enumerate() {
            let size = limits.check(value).map_err(AdmissionFailure::Other)?;
            validate(index, value)?;
            for (index, amount) in [(2, size.nodes), (3, size.payload_bytes)] {
                let (resource, limit) = self.resources()[index];
                charge[index] =
                    charge[index]
                        .checked_add(amount)
                        .ok_or(AdmissionFailure::Other(BWErr::ResourceLimit {
                            resource,
                            limit: limit as u64,
                        }))?;
            }
        }
        self.reserve(charge).map_err(AdmissionFailure::Other)
    }

    pub(super) fn value(&self, size: ValueSize) -> Result<Reservation, BWErr> {
        self.reserve([0, 1, size.nodes, size.payload_bytes, 0, 0, 0, 0, 0])
    }

    pub(super) fn diagnostic(
        &self,
        size: DiagnosticSize,
        mut previous: Option<Reservation>,
    ) -> Result<Reservation, Box<(BWErr, Option<Reservation>)>> {
        let charge = [
            0,
            0,
            0,
            0,
            size.diagnostics,
            size.call_frames,
            size.related_locations,
            size.text_bytes,
            size.source_bytes,
        ];
        if let Some(reservation) = &mut previous {
            if let Err(error) = self.replace(reservation.charge, charge) {
                return Err(Box::new((error, previous)));
            }
            reservation.charge = charge;
            Ok(previous.unwrap())
        } else {
            self.reserve(charge)
                .map_err(|error| Box::new((error, None)))
        }
    }
}

#[derive(Debug)]
pub(super) struct Reservation {
    budget: OperationBudget,
    charge: Counts,
}

impl Reservation {
    pub(super) fn merge(left: Option<Self>, right: Option<Self>) -> Option<Self> {
        match (left, right) {
            (Some(mut left), Some(mut right)) => {
                assert!(Arc::ptr_eq(&left.budget.0, &right.budget.0));
                for (charge, other) in left.charge.iter_mut().zip(&mut right.charge) {
                    *charge += *other; // Their sum is already admitted in the same tracker.
                    *other = 0;
                }
                Some(left)
            }
            (left, right) => left.or(right),
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = self
            .budget
            .0
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (count, charge) in used.iter_mut().zip(self.charge) {
            *count -= charge;
        }
    }
}

// Drop payloads before their reservations and last invocation owner.
pub(super) struct Tracked<T> {
    pub value: T,
    pub reservation: Option<Reservation>,
    pub _scope: Option<Arc<Reservation>>,
}

impl<T> Tracked<T> {
    pub(super) fn into_inner(self) -> T {
        self.value
    }
}

pub(super) struct Admission {
    pub values: Owned<Vec<Literal>>,
    pub scope: Arc<Reservation>,
}

pub(super) enum AdmissionFailure {
    Arity,
    Other(BWErr),
    Numeric(ArithmeticFailure),
    Argument {
        index: usize,
        kind: crate::core::signature::ValueKind,
    },
}

pub(super) type OperationResult = Result<Tracked<Owned<Literal>>, Box<Tracked<OwnedDiagnostic>>>;
