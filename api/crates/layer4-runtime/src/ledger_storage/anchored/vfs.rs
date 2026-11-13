//! Restricted SQLite VFS for a newly created, detached held import only.
//! SQL, pager and rollback transactions remain SQLite's; all filesystem names
//! are fixed components resolved relative to the retained directory descriptor.
use super::super::storage_error;
use crate::failure::Result;
use rusqlite::ffi;
use std::ffi::{c_char, c_int, CStr, CString};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::sync::Arc;

#[repr(C)]
struct Registration {
    raw: ffi::sqlite3_vfs,
    directory: Arc<File>,
}
pub(in crate::ledger_storage) struct Vfs {
    registration: Box<Registration>,
    name: CString,
}
impl Vfs {
    pub(super) fn new(directory: Arc<File>) -> Result<Self> {
        unsafe {
            if ffi::sqlite3_initialize() != ffi::SQLITE_OK {
                return Err(storage_error());
            }
            let parent = ffi::sqlite3_vfs_find(std::ptr::null());
            if parent.is_null() {
                return Err(storage_error());
            }
            let name = CString::new(format!("held-import-{}", uuid::Uuid::new_v4()))
                .map_err(|_| storage_error())?;
            let mut raw = *parent;
            raw.zName = name.as_ptr();
            raw.szOsFile = std::mem::size_of::<super::io::Handle>() as c_int;
            raw.xOpen = Some(open);
            raw.xDelete = Some(delete);
            raw.xAccess = Some(access);
            raw.xFullPathname = Some(full_path);
            let mut registration = Box::new(Registration { raw, directory });
            if ffi::sqlite3_vfs_register(&mut registration.raw, 0) != ffi::SQLITE_OK {
                return Err(storage_error());
            }
            Ok(Self { registration, name })
        }
    }
    pub(super) fn name(&self) -> &str {
        self.name.to_str().expect("generated ASCII VFS name")
    }
}
impl Drop for Vfs {
    fn drop(&mut self) {
        // The worker closes its connection and every handle before unregistering.
        if unsafe { ffi::sqlite3_vfs_unregister(&mut self.registration.raw) } != ffi::SQLITE_OK {
            tracing::error!(
                phase = "unregister_held_import_vfs",
                "ledger cleanup failed"
            );
        }
    }
}
unsafe fn component(name: *const c_char) -> Option<&'static CStr> {
    if name.is_null() {
        return None;
    }
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"/ledger.sqlite3" => Some(c"ledger.sqlite3"),
        b"/ledger.sqlite3-journal" => Some(c"ledger.sqlite3-journal"),
        b"/ledger.sqlite3-wal" => Some(c"ledger.sqlite3-wal"),
        b"/ledger.sqlite3-shm" => Some(c"ledger.sqlite3-shm"),
        _ => None,
    }
}
unsafe extern "C" fn full_path(
    _: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    len: c_int,
    out: *mut c_char,
) -> c_int {
    let path = c"/ledger.sqlite3".to_bytes_with_nul();
    if name.is_null()
        || unsafe { CStr::from_ptr(name) }.to_bytes() != b"ledger.sqlite3"
        || len < path.len() as c_int
    {
        return ffi::SQLITE_CANTOPEN;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(path.as_ptr().cast(), out, path.len());
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    output: *mut ffi::sqlite3_file,
    flags: c_int,
    actual: *mut c_int,
) -> c_int {
    let Some(name) = (unsafe { component(name) }) else {
        return ffi::SQLITE_CANTOPEN;
    };
    if !matches!(
        name.to_bytes(),
        b"ledger.sqlite3" | b"ledger.sqlite3-journal"
    ) || flags & ffi::SQLITE_OPEN_DELETEONCLOSE != 0
    {
        return ffi::SQLITE_CANTOPEN;
    }
    let directory = unsafe { &(*vfs.cast::<Registration>()).directory };
    let mut mode = if flags & ffi::SQLITE_OPEN_READWRITE != 0 {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    if flags & ffi::SQLITE_OPEN_CREATE != 0 {
        mode |= libc::O_CREAT;
    }
    if flags & ffi::SQLITE_OPEN_EXCLUSIVE != 0 {
        mode |= libc::O_EXCL;
    }
    let file = match super::file(directory, name, mode) {
        Ok(file) => file,
        Err(_) => return ffi::SQLITE_CANTOPEN,
    };
    unsafe {
        output
            .cast::<super::io::Handle>()
            .write(super::io::Handle::new(file, Arc::clone(directory), name));
        if !actual.is_null() {
            *actual = flags;
        }
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    _: c_int,
    exists: *mut c_int,
) -> c_int {
    let Some(name) = (unsafe { component(name) }) else {
        return ffi::SQLITE_IOERR_ACCESS;
    };
    let directory = unsafe { &(*vfs.cast::<Registration>()).directory };
    let mut meta = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            meta.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
            return ffi::SQLITE_IOERR_ACCESS;
        }
        unsafe {
            *exists = 0;
        }
    } else {
        let meta = unsafe { meta.assume_init() };
        if meta.st_mode & libc::S_IFMT != libc::S_IFREG
            || meta.st_nlink != 1
            || meta.st_mode & 0o077 != 0
            || meta.st_uid != unsafe { libc::geteuid() }
        {
            return ffi::SQLITE_IOERR_ACCESS;
        }
        unsafe {
            *exists = i32::from(meta.st_size != 0);
        }
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn delete(vfs: *mut ffi::sqlite3_vfs, name: *const c_char, sync: c_int) -> c_int {
    if unsafe { component(name) } != Some(c"ledger.sqlite3-journal") {
        return ffi::SQLITE_IOERR_DELETE;
    }
    let directory = unsafe { &(*vfs.cast::<Registration>()).directory };
    if unsafe { libc::unlinkat(directory.as_raw_fd(), c"ledger.sqlite3-journal".as_ptr(), 0) } != 0
    {
        return ffi::SQLITE_IOERR_DELETE;
    }
    if sync != 0 && directory.sync_all().is_err() {
        return ffi::SQLITE_IOERR_DIR_FSYNC;
    }
    ffi::SQLITE_OK
}
