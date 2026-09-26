use super::*;
use std::{
    borrow::Borrow,
    hash::{Hash, Hasher},
};

#[cfg(test)]
mod tests;

/// Immutable variable-name storage; Context/module snapshots share allocations.
#[derive(Clone, Debug)]
pub struct RetainedNameLimits {
    pub names: usize,
    pub name_bytes: usize,
    pub total_bytes: usize,
}

impl Default for RetainedNameLimits {
    fn default() -> Self {
        Self {
            names: 65_536,
            name_bytes: 65_536,
            total_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub(super) struct RetainedNames {
    limits: RetainedNameLimits,
    used: Mutex<[usize; 2]>,
}

impl RetainedNames {
    pub(super) fn new(limits: RetainedNameLimits) -> Self {
        Self {
            limits,
            used: Mutex::new([0; 2]),
        }
    }

    fn check_length(&self, bytes: usize) -> Result<(), BWErr> {
        if bytes > self.limits.name_bytes {
            return Err(BWErr::ResourceLimit {
                resource: "variable name bytes",
                limit: self.limits.name_bytes as u64,
            });
        }
        Ok(())
    }

    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<NameReservation, BWErr> {
        self.check_length(bytes)?;
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let names = used[0]
            .checked_add(1)
            .filter(|names| *names <= self.limits.names)
            .ok_or(BWErr::ResourceLimit {
                resource: "retained variable names",
                limit: self.limits.names as u64,
            })?;
        let total = used[1]
            .checked_add(bytes)
            .filter(|total| *total <= self.limits.total_bytes)
            .ok_or(BWErr::ResourceLimit {
                resource: "retained variable name bytes",
                limit: self.limits.total_bytes as u64,
            })?;
        *used = [names, total];
        Ok(NameReservation {
            owner: Arc::clone(self),
            bytes,
        })
    }
}

#[derive(Debug)]
struct NameReservation {
    owner: Arc<RetainedNames>,
    bytes: usize,
}

impl Drop for NameReservation {
    fn drop(&mut self) {
        let mut used = self.owner.used.lock().unwrap_or_else(|e| e.into_inner());
        used[0] -= 1;
        used[1] -= self.bytes;
    }
}

#[derive(Debug)]
struct NameData {
    // Drop the string before returning its allowance.
    text: String,
    _reservation: Option<NameReservation>,
}

#[derive(Clone, Debug)]
pub(crate) struct RetainedName(Arc<NameData>);

impl RetainedName {
    // Called only after aggregate result admission; shared names require a copy.
    pub(crate) fn into_string(self) -> String {
        match Arc::try_unwrap(self.0) {
            Ok(owned) => owned.text,
            Err(shared) => shared.text.clone(),
        }
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0.text
    }
    pub(crate) fn untracked(text: &str) -> Self {
        Self(Arc::new(NameData {
            text: text.into(),
            _reservation: None,
        }))
    }
}

impl Borrow<str> for RetainedName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}
impl PartialEq for RetainedName {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for RetainedName {}
impl Hash for RetainedName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

// Test fixtures that deliberately bypass interpreter installation can still
// construct keys; production key creation always uses Context admission.
#[cfg(test)]
impl From<&str> for RetainedName {
    fn from(value: &str) -> Self {
        Self::untracked(value)
    }
}

impl RunBudget {
    pub(crate) fn check_name_length(&self, bytes: usize) -> DiagnosticResult<()> {
        self.checkpoint()?;
        self.0
            .retained_names
            .check_length(bytes)
            .map_err(|error| self.stop(error))
    }

    pub(crate) fn retain_name(&self, name: &str) -> DiagnosticResult<RetainedName> {
        self.checkpoint()?;
        let reservation = self
            .0
            .retained_names
            .reserve(name.len())
            .map_err(|error| self.stop(error))?;
        Ok(RetainedName(Arc::new(NameData {
            text: name.into(),
            _reservation: Some(reservation),
        })))
    }
}
