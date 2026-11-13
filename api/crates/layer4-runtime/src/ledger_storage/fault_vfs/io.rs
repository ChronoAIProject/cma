use super::{Event, State};
use rusqlite::ffi;
use std::ffi::{c_int, c_void};

#[repr(C)]
pub(super) struct File {
    pub(super) base: ffi::sqlite3_file,
    pub(super) actual: *mut ffi::sqlite3_file,
    pub(super) state: *const State,
    pub(super) database: bool,
}

pub(super) static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
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

// Only SQLite calls these callbacks, with the initialized header from xOpen.
// Forward all ordinary I/O and locking to the original filesystem implementation.
unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xClose.unwrap()(f.actual)
    }
}
unsafe extern "C" fn read(
    file: *mut ffi::sqlite3_file,
    buf: *mut c_void,
    len: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xRead.unwrap()(f.actual, buf, len, offset)
    }
}
unsafe extern "C" fn write(
    file: *mut ffi::sqlite3_file,
    buf: *const c_void,
    len: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        if (*f.state).observe(if f.database {
            Event::DatabaseWrite
        } else {
            Event::JournalWrite
        }) {
            return ffi::SQLITE_FULL;
        }
        (*(*f.actual).pMethods).xWrite.unwrap()(f.actual, buf, len, offset)
    }
}
unsafe extern "C" fn truncate(file: *mut ffi::sqlite3_file, len: i64) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xTruncate.unwrap()(f.actual, len)
    }
}
unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        if (*f.state).observe(if f.database {
            Event::DatabaseSync
        } else {
            Event::JournalSync
        }) {
            return ffi::SQLITE_IOERR_FSYNC;
        }
        (*(*f.actual).pMethods).xSync.unwrap()(f.actual, flags)
    }
}
unsafe extern "C" fn size(file: *mut ffi::sqlite3_file, len: *mut i64) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xFileSize.unwrap()(f.actual, len)
    }
}
unsafe extern "C" fn lock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xLock.unwrap()(f.actual, level)
    }
}
unsafe extern "C" fn unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xUnlock.unwrap()(f.actual, level)
    }
}
unsafe extern "C" fn reserved(file: *mut ffi::sqlite3_file, result: *mut c_int) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xCheckReservedLock.unwrap()(f.actual, result)
    }
}
unsafe extern "C" fn control(file: *mut ffi::sqlite3_file, op: c_int, arg: *mut c_void) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xFileControl.unwrap()(f.actual, op, arg)
    }
}
unsafe extern "C" fn sector(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xSectorSize.unwrap()(f.actual)
    }
}
unsafe extern "C" fn characteristics(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let f = &*file.cast::<File>();
        (*(*f.actual).pMethods).xDeviceCharacteristics.unwrap()(f.actual)
    }
}
