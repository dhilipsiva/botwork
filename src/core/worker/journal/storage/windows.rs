//! The journal on Windows: a directory owned by this user whose access list
//! grants only this user, with files opened relative to its handle and never
//! through a reparse point.
use super::{invalid, Access};
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::{FileExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
            FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::{
            LocalFree, RtlNtStatusToDosError, ERROR_NO_MORE_FILES, HANDLE, NTSTATUS, UNICODE_STRING,
        },
        Security::{
            AclSizeInformation,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
            EqualSid, GetAce, GetAclInformation, GetTokenInformation, TokenUser,
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::{
            FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx, LockFileEx, BY_HANDLE_FILE_INFORMATION,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_ID_BOTH_DIR_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_TRAVERSE, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            SYNCHRONIZE,
        },
        System::{
            Threading::{GetCurrentProcess, OpenProcessToken},
            IO::{IO_STATUS_BLOCK, OVERLAPPED},
        },
    },
};

/// The access-control entry types a private access list may hold.
const ACCESS_ALLOWED: u8 = 0;
const ACCESS_DENIED: u8 = 1;
/// `IO_STATUS_BLOCK::Information` after `NtCreateFile` made a new file.
const FILE_CREATED: usize = 2;

fn raw(handle: &impl AsRawHandle) -> HANDLE {
    handle.as_raw_handle() as HANDLE
}

fn status_error(status: NTSTATUS) -> io::Error {
    // SAFETY: a pure conversion of the status code.
    io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32)
}

/// Memory the system allocated with `LocalAlloc`.
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the system's LocalAlloc and is freed once.
        unsafe { LocalFree(self.0) };
    }
}

/// This process's user, whose SID owns and alone may use the journal.
struct User(Vec<u64>);
impl User {
    fn current() -> io::Result<Self> {
        let mut token = ptr::null_mut();
        // SAFETY: the pseudo-handle of this process and an output handle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the token handle was just opened and is owned from here on.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut needed = 0;
        // SAFETY: a size query with no buffer; it fails, reporting the size.
        unsafe { GetTokenInformation(raw(&token), TokenUser, ptr::null_mut(), 0, &mut needed) };
        // u64 elements keep the TOKEN_USER and its SID aligned.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let size = u32::try_from(buffer.len() * 8).map_err(|_| invalid("Oversized token"))?;
        // SAFETY: the buffer holds `size` writable bytes.
        if unsafe {
            GetTokenInformation(
                raw(&token),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut needed,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(buffer))
    }
    fn sid(&self) -> PSID {
        // SAFETY: the buffer starts with the TOKEN_USER that GetTokenInformation
        // wrote; its SID points into the same buffer.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
    fn is(&self, sid: PSID) -> bool {
        // SAFETY: both SIDs are valid for the duration of the call.
        !sid.is_null() && unsafe { EqualSid(self.sid(), sid) } != 0
    }
    /// A security descriptor that makes this user the owner and grants this
    /// user, and no one else, full access, including to files created inside.
    fn private(&self) -> io::Result<Local> {
        let mut text = ptr::null_mut();
        // SAFETY: a valid SID and an output pointer the system allocates.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let text = Local(text.cast());
        // SAFETY: a NUL-terminated wide string the system just wrote.
        let sid = unsafe {
            let start = text.0.cast::<u16>();
            let length = (0..).take_while(|&index| *start.add(index) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(start, length))
        };
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: a NUL-terminated descriptor string and an output pointer.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Local(descriptor))
    }
}

/// Whether `handle` is owned by `user` and only `user` is granted any access.
fn private(handle: &File, user: &User) -> io::Result<bool> {
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: an open handle with READ_CONTROL and output pointers into the
    // descriptor, which is freed below.
    let error = unsafe {
        GetSecurityInfo(
            raw(handle),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let _descriptor = Local(descriptor);
    // A missing access list grants everyone everything.
    if !user.is(owner) || dacl.is_null() {
        return Ok(false);
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: a valid access list and an output of the requested class's size.
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    for index in 0..size.AceCount {
        let mut entry = ptr::null_mut();
        // SAFETY: the index is within the access list's entry count.
        if unsafe { GetAce(dacl, index, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: every entry begins with its header.
        let kind = unsafe { (*entry.cast::<ACE_HEADER>()).AceType };
        match kind {
            // Denials only take access away.
            ACCESS_DENIED => {}
            ACCESS_ALLOWED => {
                // SAFETY: an allowing entry's SID starts at SidStart.
                let sid =
                    unsafe { ptr::addr_of_mut!((*entry.cast::<ACCESS_ALLOWED_ACE>()).SidStart) };
                if !user.is(sid.cast()) {
                    return Ok(false);
                }
            }
            // Conditional, object, and other entries are not expected here.
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: an open handle and an output structure.
    if unsafe { GetFileInformationByHandle(raw(file), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information)
}

/// Open `name` in the directory `root` with `NtCreateFile`, the Windows
/// equivalent of `openat`: a rename of the directory's path cannot redirect
/// it, and a final reparse point is opened as itself rather than followed.
fn open(
    root: HANDLE,
    name: &OsStr,
    access: u32,
    disposition: u32,
    options: u32,
    security: Option<&Local>,
) -> io::Result<(File, bool)> {
    let wide: Vec<u16> = name.encode_wide().collect();
    // One component only: no separators, streams, or dot names.
    if wide.is_empty()
        || name == "."
        || name == ".."
        || wide.iter().any(|&unit| {
            unit == u16::from(b'\\') || unit == u16::from(b'/') || unit == u16::from(b':')
        })
    {
        return Err(invalid("Invalid journal name"));
    }
    let length = u16::try_from(wide.len() * 2).map_err(|_| invalid("Invalid journal name"))?;
    let object = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: wide.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root,
        ObjectName: &object,
        Attributes: 0,
        SecurityDescriptor: security.map_or(ptr::null(), |descriptor| descriptor.0 as *const _),
        SecurityQualityOfService: ptr::null(),
    };
    let mut handle = ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: every pointer refers to a live local for the call's duration.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut status,
            ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            // No FILE_SHARE_DELETE: nothing renames or removes an open entry.
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            disposition,
            options | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            ptr::null(),
            0,
        )
    };
    if result < 0 {
        return Err(status_error(result));
    }
    // SAFETY: NtCreateFile succeeded, so the handle is new and ours.
    let file = unsafe { File::from_raw_handle(handle) };
    Ok((file, status.Information == FILE_CREATED))
}

pub(super) fn at(root: &File, name: &OsStr, access: Access) -> io::Result<File> {
    let readable = FILE_GENERIC_READ;
    let writable = FILE_GENERIC_READ | FILE_GENERIC_WRITE;
    let (access, disposition, security) = match access {
        Access::Create => (writable, FILE_CREATE, true),
        Access::Lock => (writable, FILE_OPEN_IF, true),
        Access::Read => (readable, FILE_OPEN, false),
    };
    let security = security.then(|| User::current()?.private()).transpose()?;
    open(
        raw(root),
        name,
        access,
        disposition,
        FILE_NON_DIRECTORY_FILE,
        security.as_ref(),
    )
    .map(|(file, _)| file)
}

pub(in super::super) fn validate(file: &File, size: Option<u64>) -> io::Result<()> {
    let information = information(file)?;
    let length = u64::from(information.nFileSizeHigh) << 32 | u64::from(information.nFileSizeLow);
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || information.nNumberOfLinks != 1
        || size.is_some_and(|expected| length != expected)
        || !private(file, &User::current()?)?
    {
        return Err(invalid("Journal file must be private, owned, regular, and have one link with the expected size"));
    }
    Ok(())
}

pub(super) fn open_root(parent: &Path, name: &OsStr) -> io::Result<File> {
    let user = User::current()?;
    let security = user.private()?;
    let directory = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_TRAVERSE | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(parent)?;
    let (root, created) = open(
        raw(&directory),
        name,
        // Writing lets the directory's entries be flushed to disk.
        FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        FILE_OPEN_IF,
        FILE_DIRECTORY_FILE,
        Some(&security),
    )?;
    let attributes = information(&root)?.dwFileAttributes;
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || !private(&root, &user)?
    {
        return Err(invalid(
            "Journal directory must be private and owned by this user",
        ));
    }
    if created {
        // Make the new directory's own entry durable, as Linux does by
        // syncing the parent.
        OpenOptions::new()
            .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(parent)?
            .sync_all()?;
    }
    Ok(root)
}

pub(super) fn lock(file: &File) -> io::Result<()> {
    let mut overlapped = OVERLAPPED::default();
    // SAFETY: a synchronous handle; the call returns at once with the result.
    if unsafe {
        LockFileEx(
            raw(file),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn each_name(
    root: &File,
    mut visit: impl FnMut(&OsStr) -> io::Result<()>,
) -> io::Result<()> {
    // u64 elements keep each entry aligned. Reading through the handle lists
    // the directory that is open, even after a rename of its path.
    let mut buffer = vec![0u64; 8 * 1024];
    let mut class = FileIdBothDirectoryRestartInfo;
    loop {
        // SAFETY: the buffer holds the stated number of writable bytes.
        if unsafe {
            GetFileInformationByHandleEx(
                raw(root),
                class,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(());
            }
            return Err(error);
        }
        class = FileIdBothDirectoryInfo;
        let mut offset = 0;
        loop {
            // SAFETY: the system wrote a chain of entries starting at the
            // buffer, each within it and linked by its next-entry offset.
            let (next, name) = unsafe {
                let entry = buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>();
                let name = std::slice::from_raw_parts(
                    ptr::addr_of!((*entry).FileName).cast::<u16>(),
                    (*entry).FileNameLength as usize / 2,
                );
                ((*entry).NextEntryOffset, OsString::from_wide(name))
            };
            if name != "." && name != ".." {
                visit(&name)?;
            }
            if next == 0 {
                break;
            }
            offset += next as usize;
        }
    }
}

pub(in super::super) fn write_all_at(
    file: &File,
    mut bytes: &[u8],
    mut offset: u64,
) -> io::Result<()> {
    while !bytes.is_empty() {
        match file.seek_write(bytes, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => {
                bytes = &bytes[written..];
                offset += written as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(in super::super) fn read_exact_at(
    file: &File,
    mut bytes: &mut [u8],
    mut offset: u64,
) -> io::Result<()> {
    while !bytes.is_empty() {
        match file.seek_read(bytes, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => {
                bytes = &mut bytes[read..];
                offset += read as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(in super::super) fn random(bytes: &mut [u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len()).map_err(|_| invalid("Oversized random request"))?;
    // SAFETY: the buffer holds `length` writable bytes.
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(status_error(status));
    }
    Ok(())
}
