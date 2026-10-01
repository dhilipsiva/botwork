//! The journal on Linux: a mode 0700 directory owned by this user, with files
//! opened relative to its descriptor and never through a link.
use super::{invalid, Access};
use std::{
    ffi::{CString, OsStr},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};

fn open(root: &File, name: &OsStr, flags: i32) -> io::Result<File> {
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
pub(super) fn at(root: &File, name: &OsStr, access: Access) -> io::Result<File> {
    let flags = match access {
        Access::Create => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        Access::Lock => libc::O_RDWR | libc::O_CREAT,
        Access::Read => libc::O_RDONLY,
    };
    open(root, name, flags)
}
pub(in super::super) fn validate(file: &File, size: Option<u64>) -> io::Result<()> {
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
pub(super) fn open_root(parent: &Path, name: &OsStr) -> io::Result<File> {
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
    let root = open(&parent, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    let metadata = root.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(invalid(
            "Journal directory must be private and owned by this user",
        ));
    }
    parent.sync_all()?;
    Ok(root)
}
pub(super) fn lock(file: &File) -> io::Result<()> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub(super) fn each_name(
    root: &File,
    mut visit: impl FnMut(&OsStr) -> io::Result<()>,
) -> io::Result<()> {
    // proc follows our live directory descriptor, even after a rename. All
    // record opens remain relative to that descriptor, not the path.
    for entry in std::fs::read_dir(format!("/proc/self/fd/{}", root.as_raw_fd()))? {
        visit(&entry?.file_name())?;
    }
    Ok(())
}
pub(in super::super) fn write_all_at(file: &File, bytes: &[u8], offset: u64) -> io::Result<()> {
    file.write_all_at(bytes, offset)
}
pub(in super::super) fn read_exact_at(
    file: &File,
    bytes: &mut [u8],
    offset: u64,
) -> io::Result<()> {
    file.read_exact_at(bytes, offset)
}
pub(in super::super) fn random(bytes: &mut [u8]) -> io::Result<()> {
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
