use super::*;
use crate::core::value_limits::ValueSize;
use std::ops::Deref;

#[cfg(test)]
mod tests;

/// Aggregate live Literal temporaries during synchronous DSL evaluation.
#[derive(Clone, Debug)]
pub struct TemporaryLimits {
    pub values: usize,
    pub nodes: usize,
    pub payload_bytes: usize,
}

impl Default for TemporaryLimits {
    fn default() -> Self {
        Self {
            values: 65_536,
            nodes: 262_144,
            payload_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub(super) struct TemporaryValues {
    limits: TemporaryLimits,
    used: Mutex<[usize; 3]>,
}

impl TemporaryValues {
    pub(super) fn new(limits: TemporaryLimits) -> Self {
        Self {
            limits,
            used: Mutex::new([0; 3]),
        }
    }

    fn reserve(self: &Arc<Self>, size: ValueSize) -> Result<TemporaryReservation, BWErr> {
        self.reserve_counts([1, size.nodes, size.payload_bytes])
    }

    fn reserve_counts(self: &Arc<Self>, charge: [usize; 3]) -> Result<TemporaryReservation, BWErr> {
        let mut used = self.used.lock().unwrap_or_else(|error| error.into_inner());
        let mut next = *used;
        for (index, (resource, limit)) in [
            ("temporary values", self.limits.values),
            ("temporary value nodes", self.limits.nodes),
            ("temporary value payload bytes", self.limits.payload_bytes),
        ]
        .into_iter()
        .enumerate()
        {
            next[index] = next[index]
                .checked_add(charge[index])
                .filter(|value| *value <= limit)
                .ok_or(BWErr::ResourceLimit {
                    resource,
                    limit: limit as u64,
                })?;
        }
        *used = next;
        Ok(TemporaryReservation {
            owner: Arc::clone(self),
            charge,
        })
    }
}

#[derive(Debug)]
pub(crate) struct TemporaryReservation {
    owner: Arc<TemporaryValues>,
    charge: [usize; 3],
}

impl TemporaryReservation {
    /// Admit streaming nodes/payload without inventing another live value handle.
    pub(crate) fn grow(&mut self, nodes: usize, bytes: usize) -> Result<(), BWErr> {
        let additional = self.owner.reserve_counts([0, nodes, bytes])?;
        self.absorb(additional);
        Ok(())
    }

    pub(crate) fn release_argument_slot(&mut self) {
        let mut used = self
            .owner
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.charge[0] -= 1;
        self.charge[1] -= 1;
        used[0] -= 1;
        used[1] -= 1;
    }
    /// Move an already admitted child into its container without duplicating nodes.
    pub(crate) fn absorb(&mut self, mut child: Self) {
        assert!(
            Arc::ptr_eq(&self.owner, &child.owner),
            "shared temporary tracker"
        );
        let mut used = self
            .owner
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        used[0] -= child.charge[0];
        self.charge[1] += child.charge[1];
        self.charge[2] += child.charge[2];
        child.charge = [0; 3];
        drop(used);
    }

    /// Release a static child placeholder or a destroyed map child's metrics.
    pub(crate) fn release_child(&mut self, size: ValueSize) {
        let mut used = self
            .owner
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.charge[1] -= size.nodes;
        self.charge[2] -= size.payload_bytes;
        used[1] -= size.nodes;
        used[2] -= size.payload_bytes;
    }
}

impl Drop for TemporaryReservation {
    fn drop(&mut self) {
        let mut used = self
            .owner
            .used
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (count, charge) in used.iter_mut().zip(self.charge) {
            *count -= charge;
        }
    }
}

#[derive(Debug)]
pub(crate) struct TemporaryValue {
    // Release the owned value before returning its live allowance.
    pub(crate) value: Literal,
    pub(crate) reservation: Option<TemporaryReservation>,
}

impl TemporaryValue {
    pub(crate) fn new(value: Literal, reservation: Option<TemporaryReservation>) -> Self {
        Self { value, reservation }
    }
    pub(crate) fn into_parts(self) -> (Literal, Option<TemporaryReservation>) {
        (self.value, self.reservation)
    }
    /// Transfer to a public host-owned return boundary.
    pub(crate) fn into_inner(self) -> Literal {
        self.value
    }
    pub(crate) fn absorb(&mut self, child: Option<TemporaryReservation>) {
        if let (Some(parent), Some(child)) = (&mut self.reservation, child) {
            parent.absorb(child);
        }
    }
    pub(crate) fn release_child(&mut self, size: ValueSize) {
        if let Some(reservation) = &mut self.reservation {
            reservation.release_child(size);
        }
    }
}

impl Deref for TemporaryValue {
    type Target = Literal;
    fn deref(&self) -> &Literal {
        &self.value
    }
}

impl RunBudget {
    pub(crate) fn reserve_argument_slots(
        &self,
        count: usize,
    ) -> DiagnosticResult<TemporaryReservation> {
        self.checkpoint()?;
        self.0
            .temporary_values
            .reserve_counts([count, count, 0])
            .map_err(|error| self.stop(error))
    }
    pub(crate) fn reserve_temporary(
        &self,
        size: ValueSize,
    ) -> DiagnosticResult<TemporaryReservation> {
        self.checkpoint()?;
        self.0
            .temporary_values
            .reserve(size)
            .map_err(|error| self.stop(error))
    }
}
