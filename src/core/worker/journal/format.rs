//! Versioned fixed slots: intent, publication, guardian receipt, reconciliation.
use super::storage::{read_exact_at, write_all_at};
use super::*;
use std::fs::File;

const SLOT_BYTES: usize = 64;
pub(super) const FILE_BYTES: usize = SLOT_BYTES * 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Role {
    Intent,
    Published,
    Guardian,
    Reconciled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Frame {
    pub id: JournalId,
    pub role: Role,
    pub host: Option<JournalMetadata>,
    pub guardian: Option<GuardianReceipt>,
}
impl Frame {
    pub fn host(id: JournalId, role: Role, metadata: JournalMetadata) -> Self {
        Self {
            id,
            role,
            host: Some(metadata),
            guardian: None,
        }
    }
}
fn crc(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
fn encode(frame: Frame) -> [u8; SLOT_BYTES] {
    let mut bytes = [0; SLOT_BYTES];
    bytes[..4].copy_from_slice(b"BWJR");
    bytes[4] = 1;
    bytes[5] = frame.role as u8;
    bytes[12..28].copy_from_slice(&frame.id.session);
    bytes[28..36].copy_from_slice(&frame.id.sequence.to_le_bytes());
    if let Some(host) = frame.host {
        bytes[6] = match host.outcome {
            WorkerOutcome::Succeeded => 1,
            WorkerOutcome::Failed => 2,
            WorkerOutcome::Cancelled => 3,
            WorkerOutcome::TimedOut => 4,
            WorkerOutcome::Interrupted => 5,
        };
        bytes[7] = match host.cleanup {
            WorkerCleanup::NotStarted => 1,
            WorkerCleanup::Reaped => 2,
            WorkerCleanup::TreeReaped => 3,
            WorkerCleanup::Pending => 4,
            WorkerCleanup::Unverified => 5,
            WorkerCleanup::NamespaceReaped => 6,
        };
        bytes[8] = u8::from(host.io_complete)
            | (u8::from(host.progress_complete) << 1)
            | (u8::from(host.exit_status.is_some()) << 2);
        bytes[36..40].copy_from_slice(&host.exit_status.unwrap_or(0).to_le_bytes());
    }
    if let Some(receipt) = frame.guardian {
        let (kind, status, errno) = match receipt {
            GuardianReceipt::NotStarted { errno } => (1, 0, errno),
            GuardianReceipt::TreeSettled { exit_status, errno } => (3, exit_status, errno),
        };
        bytes[7] = kind;
        bytes[36..40].copy_from_slice(&status.to_le_bytes());
        bytes[40..44].copy_from_slice(&errno.to_le_bytes());
    }
    let checksum = crc(&bytes[..60]);
    bytes[60..].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

/// A wait status on Linux.
#[cfg(target_os = "linux")]
fn valid_status(raw: i32) -> bool {
    (0..=0xff00).contains(&raw)
        && ((libc::WIFEXITED(raw) && raw & 0xff == 0)
            || (raw <= 0xff && libc::WIFSIGNALED(raw) && libc::WTERMSIG(raw) <= libc::SIGRTMAX()))
}
/// An exit code on Windows, where every 32-bit value is one.
#[cfg(windows)]
fn valid_status(_: i32) -> bool {
    true
}
fn decode(bytes: &[u8; SLOT_BYTES], role: Role) -> io::Result<Frame> {
    let bad = || invalid("Invalid journal frame");
    if &bytes[..4] != b"BWJR"
        || bytes[4] != 1
        || bytes[5] != role as u8
        || crc(&bytes[..60]) != u32::from_le_bytes(bytes[60..].try_into().unwrap())
    {
        return Err(bad());
    }
    let id = JournalId {
        session: bytes[12..28].try_into().unwrap(),
        sequence: u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
    };
    if id.sequence == 0 {
        return Err(bad());
    }
    let raw = i32::from_le_bytes(bytes[36..40].try_into().unwrap());
    let errno = i32::from_le_bytes(bytes[40..44].try_into().unwrap());
    let mut frame = Frame {
        id,
        role,
        host: None,
        guardian: None,
    };
    match role {
        Role::Intent => {}
        Role::Published | Role::Reconciled => {
            let outcome = match bytes[6] {
                1 => WorkerOutcome::Succeeded,
                2 => WorkerOutcome::Failed,
                3 => WorkerOutcome::Cancelled,
                4 => WorkerOutcome::TimedOut,
                5 => WorkerOutcome::Interrupted,
                _ => return Err(bad()),
            };
            let cleanup = match bytes[7] {
                1 => WorkerCleanup::NotStarted,
                2 => WorkerCleanup::Reaped,
                3 => WorkerCleanup::TreeReaped,
                4 => WorkerCleanup::Pending,
                5 => WorkerCleanup::Unverified,
                6 => WorkerCleanup::NamespaceReaped,
                _ => return Err(bad()),
            };
            let host = JournalMetadata {
                outcome,
                cleanup,
                exit_status: (bytes[8] & 4 != 0).then_some(raw),
                io_complete: bytes[8] & 1 != 0,
                progress_complete: bytes[8] & 2 != 0,
            };
            if host.exit_status.is_some_and(|status| !valid_status(status))
                || (host.io_complete && !host.progress_complete)
                || (matches!(cleanup, WorkerCleanup::Pending | WorkerCleanup::Unverified)
                    && host.progress_complete)
                || (cleanup == WorkerCleanup::NotStarted
                    && (host.exit_status.is_some() || host.io_complete))
                || (outcome == WorkerOutcome::Succeeded
                    && (cleanup != WorkerCleanup::TreeReaped
                        || !host.io_complete
                        || !host.progress_complete
                        || host.exit_status != Some(0)))
            {
                return Err(bad());
            }
            frame.host = Some(host);
        }
        Role::Guardian => {
            frame.guardian = Some(match bytes[7] {
                1 if raw == 0 && errno > 0 => GuardianReceipt::NotStarted { errno },
                3 if valid_status(raw) && errno >= 0 => GuardianReceipt::TreeSettled {
                    exit_status: raw,
                    errno,
                },
                _ => return Err(bad()),
            });
        }
    }
    // Canonical re-encoding checks every reserved byte, flag, and absent field.
    if encode(frame) != *bytes {
        return Err(bad());
    }
    Ok(frame)
}
pub(super) fn write(file: &File, frame: Frame) -> io::Result<()> {
    write_all_at(file, &encode(frame), frame.role as u64 * SLOT_BYTES as u64)
}
pub(super) fn read_frame(file: &File, role: Role) -> io::Result<Option<Frame>> {
    let mut bytes = [0; SLOT_BYTES];
    read_exact_at(file, &mut bytes, role as u64 * SLOT_BYTES as u64)?;
    if bytes == [0; SLOT_BYTES] {
        Ok(None)
    } else {
        decode(&bytes, role).map(Some)
    }
}

pub(super) fn recover(
    file: &File,
    id: JournalId,
    current: [u8; 16],
) -> io::Result<RecoveredWorker> {
    let mut recovered = RecoveredWorker {
        id,
        published: None,
        reconciled: None,
        guardian: None,
        damaged: file.metadata()?.len() != FILE_BYTES as u64,
        outcome: None,
    };
    for role in [
        Role::Intent,
        Role::Published,
        Role::Guardian,
        Role::Reconciled,
    ] {
        match read_frame(file, role) {
            Ok(Some(frame)) if frame.id == id => match role {
                Role::Intent => {}
                Role::Published => recovered.published = frame.host,
                Role::Reconciled => recovered.reconciled = frame.host,
                Role::Guardian => recovered.guardian = frame.guardian,
            },
            Ok(None) if role != Role::Intent => {}
            _ => recovered.damaged = true,
        }
    }
    if let (Some(published), Some(reconciled)) = (recovered.published, recovered.reconciled) {
        if published.outcome != reconciled.outcome
            || (published.cleanup != WorkerCleanup::Pending && published != reconciled)
            || (published.exit_status.is_some() && published.exit_status != reconciled.exit_status)
        {
            recovered.damaged = true;
        }
    }
    if let Some(host) = recovered.reconciled.or(recovered.published) {
        if let Some(receipt) = recovered.guardian {
            let consistent = match receipt {
                GuardianReceipt::NotStarted { .. } => {
                    host.cleanup == WorkerCleanup::NotStarted
                        || host.cleanup == WorkerCleanup::Pending
                        || host.cleanup == WorkerCleanup::Unverified
                }
                GuardianReceipt::TreeSettled { exit_status, errno } => {
                    host.cleanup != WorkerCleanup::NotStarted
                        && host.exit_status.is_none_or(|raw| raw == exit_status)
                        && (host.outcome != WorkerOutcome::Succeeded || errno == 0)
                }
            };
            if !consistent {
                recovered.damaged = true;
            }
        }
    }
    recovered.outcome = match recovered.published {
        Some(host) if host.outcome != WorkerOutcome::Succeeded => Some(host.outcome),
        Some(host)
            if !recovered.damaged
                && recovered.reconciled == Some(host)
                && matches!(
                    recovered.guardian,
                    Some(GuardianReceipt::TreeSettled {
                        exit_status: 0,
                        errno: 0
                    })
                ) =>
        {
            Some(WorkerOutcome::Succeeded)
        }
        Some(_) => Some(WorkerOutcome::Interrupted),
        None if id.session != current || recovered.damaged => Some(WorkerOutcome::Interrupted),
        None => None,
    };
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_and_reserved_bytes_are_validated() {
        assert_eq!(crc(b"123456789"), 0xcbf43926);
        let frame = Frame {
            id: JournalId {
                session: [7; 16],
                sequence: 4,
            },
            role: Role::Intent,
            host: None,
            guardian: None,
        };
        let encoded = encode(frame);
        assert_eq!(decode(&encoded, Role::Intent).unwrap(), frame);
        for index in 0..SLOT_BYTES {
            let mut damaged = encoded;
            damaged[index] ^= 1;
            assert!(decode(&damaged, Role::Intent).is_err());
        }
        let mut future = encoded;
        future[44] = 1;
        let checksum = crc(&future[..60]);
        future[60..].copy_from_slice(&checksum.to_le_bytes());
        assert!(decode(&future, Role::Intent).is_err());
    }
}
