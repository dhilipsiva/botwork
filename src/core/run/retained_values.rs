use super::*;
use crate::core::value_limits::ValueSize;

#[cfg(test)]
mod tests;

/// Aggregate live values stored in variable bindings, shared across Context clones.
/// Counts logical value content, not allocator capacity or temporary/result copies.
#[derive(Clone, Debug)]
pub struct RetainedValueLimits {
    pub values: usize,
    pub nodes: usize,
    pub payload_bytes: usize,
}

impl Default for RetainedValueLimits {
    fn default() -> Self {
        Self {
            values: 65_536,
            nodes: 262_144,
            payload_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub(super) struct RetainedValues {
    limits: RetainedValueLimits,
    used: Mutex<[usize; 3]>,
}

impl RetainedValues {
    pub(super) fn new(limits: RetainedValueLimits) -> Self {
        Self {
            limits,
            used: Mutex::new([0; 3]),
        }
    }

    fn reserve(self: &Arc<Self>, size: ValueSize) -> Result<ValueReservation, BWErr> {
        let charge = [1, size.nodes, size.payload_bytes];
        let resources = [
            ("retained values", self.limits.values),
            ("retained value nodes", self.limits.nodes),
            ("retained value payload bytes", self.limits.payload_bytes),
        ];
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = *used;
        for (index, (resource, maximum)) in resources.into_iter().enumerate() {
            next[index] = next[index]
                .checked_add(charge[index])
                .filter(|value| *value <= maximum)
                .ok_or(BWErr::ResourceLimit {
                    resource,
                    limit: maximum as u64,
                })?;
        }
        *used = next;
        Ok(ValueReservation {
            owner: Arc::clone(self),
            charge,
        })
    }
}

#[derive(Debug)]
pub(crate) struct ValueReservation {
    owner: Arc<RetainedValues>,
    charge: [usize; 3],
}

impl Drop for ValueReservation {
    fn drop(&mut self) {
        let mut used = self.owner.used.lock().unwrap_or_else(|e| e.into_inner());
        for (current, charge) in used.iter_mut().zip(self.charge) {
            *current -= charge;
        }
    }
}

#[derive(Debug)]
pub(crate) struct StoredValue {
    // Field order releases the value before returning its reservation.
    pub(crate) value: Literal,
    _reservation: Option<ValueReservation>,
}

impl StoredValue {
    pub(crate) fn new(value: Literal, reservation: Option<ValueReservation>) -> Self {
        Self {
            value,
            _reservation: reservation,
        }
    }
}

impl RunBudget {
    pub(crate) fn reserve_value(&self, size: ValueSize) -> DiagnosticResult<ValueReservation> {
        self.checkpoint()?;
        self.0
            .retained_values
            .reserve(size)
            .map_err(|error| self.stop(error))
    }
}
