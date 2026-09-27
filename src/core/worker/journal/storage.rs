use super::*;
use std::{
    ffi::{CString, OsStr},
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::Component,
};

pub(super) struct Directory {
    root: File,
    _lock: File,
    pub progress: Mutex<JournalFlush>,
    pub changed: Condvar,
    #[cfg(test)]
    pub intent_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    pub write_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

fn at(root: &File, name: &OsStr, flags: i32) -> io::Result<File> {
    let name = CString::new(name.as_bytes()).map_err(|_| invalid("Invalid journal name"))?;
    // SAFETY: root and the terminated name remain live. Returned descriptor is new.
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
pub(super) fn validate(file: &File, size: Option<u64>) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || size.is_some_and(|expected| metadata.len() != expected)
    {
        return Err(invalid("Journal file must be private, owned, regular, and have one link with the expected size"));
    }
    Ok(())
}
impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return Err(invalid(
                "Journal path must be absolute without dot components",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| invalid("Journal root cannot be filesystem root"))?;
        let name = path
            .file_name()
            .ok_or_else(|| invalid("Journal directory name is required"))?;
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(parent)?;
        let c_name = CString::new(name.as_bytes()).map_err(|_| invalid("Invalid journal path"))?;
        if unsafe { libc::mkdirat(parent.as_raw_fd(), c_name.as_ptr(), 0o700) } == -1 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        let root = at(&parent, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let metadata = root.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(invalid(
                "Journal directory must be private and owned by this user",
            ));
        }
        parent.sync_all()?;
        let lock = at(&root, OsStr::new(".lock"), libc::O_RDWR | libc::O_CREAT)?;
        validate(&lock, Some(0))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
            return Err(io::Error::last_os_error());
        }
        lock.sync_all()?;
        root.sync_all()?;
        Ok(Self {
            root,
            _lock: lock,
            progress: Mutex::new(JournalFlush::default()),
            changed: Condvar::new(),
            #[cfg(test)]
            intent_hook: Mutex::new(None),
            #[cfg(test)]
            write_hook: Mutex::new(None),
        })
    }
    pub fn ids(&self, maximum: usize) -> io::Result<Vec<JournalId>> {
        let mut ids = Vec::new();
        // proc follows our live directory descriptor, even after a rename. All
        // record opens below remain relative to that descriptor, not this path.
        for entry in std::fs::read_dir(format!("/proc/self/fd/{}", self.root.as_raw_fd()))? {
            let name = entry?.file_name();
            if name == ".lock" {
                continue;
            }
            if ids.len() == maximum {
                return Err(invalid(
                    "Journal contains more records than its configured limit",
                ));
            }
            let name = name
                .to_str()
                .ok_or_else(|| invalid("Unrecognized journal entry"))?;
            let id = parse_name(name).ok_or_else(|| invalid("Unrecognized journal entry"))?;
            ids.push(id);
        }
        ids.sort_unstable();
        Ok(ids)
    }
    pub fn create(&self, id: JournalId) -> io::Result<File> {
        let file = at(
            &self.root,
            OsStr::new(&format!("{id}.bwk")),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )?;
        validate(&file, Some(0))?;
        file.set_len(format::FILE_BYTES as u64)?;
        format::write(
            &file,
            Frame {
                id,
                role: Role::Intent,
                host: None,
                guardian: None,
            },
        )?;
        #[cfg(test)]
        Self::hook(&self.intent_hook);
        file.sync_all()?;
        self.root.sync_all()?;
        Ok(file)
    }
    pub fn read(&self, id: JournalId, session: [u8; 16]) -> io::Result<RecoveredWorker> {
        let file = at(&self.root, OsStr::new(&format!("{id}.bwk")), libc::O_RDONLY)?;
        validate(&file, None)?;
        format::recover(&file, id, session)
    }
    pub fn completed(&self, failed: bool) {
        let mut state = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        state.pending -= 1;
        state.failed += usize::from(failed);
        self.changed.notify_all();
    }
    #[cfg(test)]
    pub fn hook(hook: &Mutex<Option<Box<dyn FnOnce() + Send>>>) {
        let callback = hook.lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
    }
}
fn parse_name(name: &str) -> Option<JournalId> {
    if name.len() != 53 || !name.is_ascii() || &name[32..33] != "-" || &name[49..] != ".bwk" {
        return None;
    }
    let mut session = [0; 16];
    for (index, byte) in session.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&name[index * 2..index * 2 + 2], 16).ok()?;
    }
    let sequence = u64::from_str_radix(&name[33..49], 16).ok()?;
    let id = JournalId { session, sequence };
    (sequence != 0 && format!("{id}.bwk") == name).then_some(id)
}
pub(super) fn random(bytes: &mut [u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        // SAFETY: the remaining output slice is valid for precisely this length.
        let read = unsafe {
            libc::getrandom(bytes[offset..].as_mut_ptr().cast(), bytes.len() - offset, 0)
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Entropy source returned no bytes",
            ));
        }
        offset += read as usize;
    }
    Ok(())
}
