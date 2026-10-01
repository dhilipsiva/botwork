use super::*;
use std::{ffi::OsStr, fs::File, path::Component};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub(super) use platform::{random, read_exact_at, validate, write_all_at};

/// How a journal file is opened, always relative to the journal directory and
/// never through a link.
#[derive(Clone, Copy)]
enum Access {
    /// Create a new private file for reading and writing; fail if it exists.
    Create,
    /// Open the lock file for reading and writing, creating it privately.
    Lock,
    /// Open an existing file for reading.
    Read,
}

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
        let root = platform::open_root(parent, name)?;
        let lock = platform::at(&root, OsStr::new(".lock"), Access::Lock)?;
        validate(&lock, Some(0))?;
        platform::lock(&lock)?;
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
        platform::each_name(&self.root, |name| {
            if name == ".lock" {
                return Ok(());
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
            Ok(())
        })?;
        ids.sort_unstable();
        Ok(ids)
    }
    pub fn create(&self, id: JournalId) -> io::Result<File> {
        let file = self.new_record(id)?;
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
    /// An empty private record file, as an interrupted intent leaves behind.
    fn new_record(&self, id: JournalId) -> io::Result<File> {
        platform::at(&self.root, OsStr::new(&format!("{id}.bwk")), Access::Create)
    }
    pub fn read(&self, id: JournalId, session: [u8; 16]) -> io::Result<RecoveredWorker> {
        let file = platform::at(&self.root, OsStr::new(&format!("{id}.bwk")), Access::Read)?;
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
    /// Leave an empty record behind, as a host lost while persisting intent
    /// does.
    #[cfg(test)]
    pub fn plant(&self, id: JournalId) -> io::Result<File> {
        self.new_record(id)
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
