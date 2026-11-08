//! Unix descriptor I/O for the restricted held-import VFS. Database byte locks
//! use SQLite's standard POSIX ranges; the outer writer.lock excludes other
//! import connections. WAL and shared-memory methods are deliberately absent.
use rusqlite::ffi;
use std::ffi::{c_int, c_void, CStr};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::sync::Arc;
const PENDING: i64 = 0x40000000;
const RESERVED: i64 = PENDING + 1;
const SHARED: i64 = PENDING + 2;
#[repr(C)]
pub(super) struct Handle {
    base: ffi::sqlite3_file,
    file: File,
    directory: Arc<File>,
    name: &'static CStr,
    level: c_int,
}
impl Handle {
    pub(super) fn new(file: File, directory: Arc<File>, name: &'static CStr) -> Self {
        Self {
            base: ffi::sqlite3_file { pMethods: &METHODS },
            file,
            directory,
            name,
            level: ffi::SQLITE_LOCK_NONE,
        }
    }
    fn range(&self, kind: c_int, start: i64, length: i64) -> c_int {
        let mut lock = unsafe { std::mem::zeroed::<libc::flock>() };
        lock.l_type = kind as _;
        lock.l_whence = libc::SEEK_SET as _;
        lock.l_start = start;
        lock.l_len = length;
        if unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_SETLK, &lock) } == 0 {
            ffi::SQLITE_OK
        } else {
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EAGAIN | libc::EACCES) => ffi::SQLITE_BUSY,
                _ => ffi::SQLITE_IOERR_LOCK,
            }
        }
    }
}
static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(size),
    xLock: Some(lock),
    xUnlock: Some(unlock),
    xCheckReservedLock: Some(reserved),
    xFileControl: Some(control),
    xSectorSize: Some(sector),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};
unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        std::ptr::drop_in_place(file.cast::<Handle>());
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn read(
    file: *mut ffi::sqlite3_file,
    data: *mut c_void,
    len: c_int,
    offset: i64,
) -> c_int {
    if len < 0 || offset < 0 {
        return ffi::SQLITE_IOERR_READ;
    }
    let f = unsafe { &*file.cast::<Handle>() };
    let bytes = unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), len as usize) };
    let mut done = 0;
    while done < bytes.len() {
        match f
            .file
            .read_at(&mut bytes[done..], offset as u64 + done as u64)
        {
            Ok(0) => {
                bytes[done..].fill(0);
                return ffi::SQLITE_IOERR_SHORT_READ;
            }
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return ffi::SQLITE_IOERR_READ,
        }
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn write(
    file: *mut ffi::sqlite3_file,
    data: *const c_void,
    len: c_int,
    offset: i64,
) -> c_int {
    if len < 0 || offset < 0 {
        return ffi::SQLITE_IOERR_WRITE;
    }
    let f = unsafe { &*file.cast::<Handle>() };
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize) };
    if f.file.write_all_at(bytes, offset as u64).is_ok() {
        ffi::SQLITE_OK
    } else {
        ffi::SQLITE_IOERR_WRITE
    }
}
unsafe extern "C" fn truncate(file: *mut ffi::sqlite3_file, length: i64) -> c_int {
    if length < 0 {
        return ffi::SQLITE_IOERR_TRUNCATE;
    }
    if unsafe { &*file.cast::<Handle>() }
        .file
        .set_len(length as u64)
        .is_ok()
    {
        ffi::SQLITE_OK
    } else {
        ffi::SQLITE_IOERR_TRUNCATE
    }
}
unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, _: c_int) -> c_int {
    let f = unsafe { &*file.cast::<Handle>() };
    if f.file.sync_all().is_err() {
        return ffi::SQLITE_IOERR_FSYNC;
    }
    if f.name == c"ledger.sqlite3-journal" && f.directory.sync_all().is_err() {
        return ffi::SQLITE_IOERR_DIR_FSYNC;
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn size(file: *mut ffi::sqlite3_file, length: *mut i64) -> c_int {
    match unsafe { &*file.cast::<Handle>() }.file.metadata() {
        Ok(meta) => {
            unsafe {
                *length = meta.len() as i64;
            }
            ffi::SQLITE_OK
        }
        Err(_) => ffi::SQLITE_IOERR_FSTAT,
    }
}
unsafe extern "C" fn lock(file: *mut ffi::sqlite3_file, requested: c_int) -> c_int {
    let f = unsafe { &mut *file.cast::<Handle>() };
    if requested <= f.level {
        return ffi::SQLITE_OK;
    }
    let result = match requested {
        ffi::SQLITE_LOCK_SHARED => {
            let pending = f.range(libc::F_RDLCK as _, PENDING, 1);
            if pending != ffi::SQLITE_OK {
                return pending;
            }
            let result = f.range(libc::F_RDLCK as _, SHARED, 510);
            let release = f.range(libc::F_UNLCK as _, PENDING, 1);
            if release != ffi::SQLITE_OK {
                release
            } else {
                result
            }
        }
        ffi::SQLITE_LOCK_RESERVED => f.range(libc::F_WRLCK as _, RESERVED, 1),
        ffi::SQLITE_LOCK_EXCLUSIVE => {
            let pending = f.range(libc::F_WRLCK as _, PENDING, 1);
            if pending != ffi::SQLITE_OK {
                return pending;
            }
            f.level = ffi::SQLITE_LOCK_PENDING;
            f.range(libc::F_WRLCK as _, SHARED, 510)
        }
        _ => ffi::SQLITE_IOERR_LOCK,
    };
    if result == ffi::SQLITE_OK {
        f.level = requested;
    }
    result
}
unsafe extern "C" fn unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let f = unsafe { &mut *file.cast::<Handle>() };
    let result = match level {
        ffi::SQLITE_LOCK_NONE => f.range(libc::F_UNLCK as _, PENDING, 512),
        ffi::SQLITE_LOCK_SHARED => {
            let shared = f.range(libc::F_RDLCK as _, SHARED, 510);
            if shared != ffi::SQLITE_OK {
                return shared;
            }
            f.range(libc::F_UNLCK as _, PENDING, 2)
        }
        _ => ffi::SQLITE_IOERR_UNLOCK,
    };
    if result == ffi::SQLITE_OK {
        f.level = level;
    }
    result
}
unsafe extern "C" fn reserved(file: *mut ffi::sqlite3_file, result: *mut c_int) -> c_int {
    let f = unsafe { &*file.cast::<Handle>() };
    if f.level >= ffi::SQLITE_LOCK_RESERVED {
        unsafe {
            *result = 1;
        }
        return ffi::SQLITE_OK;
    }
    let mut lock = unsafe { std::mem::zeroed::<libc::flock>() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    lock.l_start = RESERVED;
    lock.l_len = 1;
    if unsafe { libc::fcntl(f.file.as_raw_fd(), libc::F_GETLK, &mut lock) } != 0 {
        return ffi::SQLITE_IOERR_CHECKRESERVEDLOCK;
    }
    unsafe {
        let unlocked: libc::c_short = libc::F_UNLCK as _;
        *result = i32::from(lock.l_type != unlocked);
    }
    ffi::SQLITE_OK
}
unsafe extern "C" fn control(file: *mut ffi::sqlite3_file, op: c_int, value: *mut c_void) -> c_int {
    let f = unsafe { &*file.cast::<Handle>() };
    match op {
        ffi::SQLITE_FCNTL_LOCKSTATE => {
            unsafe {
                *value.cast::<c_int>() = f.level;
            }
            ffi::SQLITE_OK
        }
        ffi::SQLITE_FCNTL_HAS_MOVED => {
            let mut meta = std::mem::MaybeUninit::<libc::stat>::uninit();
            let found = unsafe {
                libc::fstatat(
                    f.directory.as_raw_fd(),
                    f.name.as_ptr(),
                    meta.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } == 0;
            let same = if found {
                let meta = unsafe { meta.assume_init() };
                #[cfg(target_os = "macos")]
                let device = meta.st_dev as u64;
                #[cfg(target_os = "linux")]
                let device = meta.st_dev;
                f.file
                    .metadata()
                    .is_ok_and(|original| original.dev() == device && original.ino() == meta.st_ino)
            } else {
                false
            };
            unsafe {
                *value.cast::<c_int>() = i32::from(!same);
            }
            ffi::SQLITE_OK
        }
        _ => ffi::SQLITE_NOTFOUND,
    }
}
unsafe extern "C" fn sector(_: *mut ffi::sqlite3_file) -> c_int {
    4096
}
unsafe extern "C" fn characteristics(_: *mut ffi::sqlite3_file) -> c_int {
    0
}
