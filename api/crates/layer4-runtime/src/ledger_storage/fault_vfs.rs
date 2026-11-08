//! Test-only SQLite VFS forwarding to the real filesystem. Each named wrapper
//! owns its fault state until every connection is explicitly closed.
mod durable;
mod io;

use rusqlite::ffi;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, CString};
use std::sync::{Arc, LazyLock, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    JournalWrite,
    DatabaseWrite,
    JournalSync,
    DatabaseSync,
    Delete(bool),
}
pub(crate) struct State {
    parent: usize,
    fault: Mutex<Option<Event>>,
    events: Mutex<Vec<Event>>,
    namespace: Mutex<Option<durable::Namespace>>,
}
impl State {
    pub(crate) fn arm(&self, event: Event) {
        *self.fault.lock().unwrap() = Some(event);
        self.events.lock().unwrap().clear();
    }
    pub(crate) fn clear(&self) {
        *self.fault.lock().unwrap() = None;
        self.events.lock().unwrap().clear();
    }
    pub(crate) fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
    fn observe(&self, event: Event) -> bool {
        self.events.lock().unwrap().push(event);
        let mut fault = self.fault.lock().unwrap();
        if *fault == Some(event) {
            *fault = None;
            true
        } else {
            false
        }
    }
}
static STATES: LazyLock<Mutex<BTreeMap<usize, Arc<State>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

pub(crate) struct Vfs {
    vfs: Box<ffi::sqlite3_vfs>,
    name: CString,
    pub(crate) state: Arc<State>,
}
impl Vfs {
    pub(crate) fn new() -> Self {
        // SQLite retains the registered struct/name pointers. Boxes/CString keep
        // addresses stable; Drop unregisters only after the test closes handles.
        unsafe {
            assert_eq!(ffi::sqlite3_initialize(), ffi::SQLITE_OK);
            let parent = ffi::sqlite3_vfs_find(std::ptr::null());
            assert!(!parent.is_null());
            let name = CString::new(format!("ledger-fault-{}", uuid::Uuid::new_v4())).unwrap();
            let mut vfs = Box::new(*parent);
            vfs.zName = name.as_ptr();
            vfs.szOsFile += std::mem::size_of::<io::File>() as c_int;
            vfs.xOpen = Some(open);
            vfs.xDelete = Some(delete);
            let state = Arc::new(State {
                parent: parent as usize,
                fault: Mutex::new(None),
                events: Mutex::new(Vec::new()),
                namespace: Mutex::new(None),
            });
            STATES
                .lock()
                .unwrap()
                .insert((&*vfs as *const _) as usize, Arc::clone(&state));
            assert_eq!(ffi::sqlite3_vfs_register(&mut *vfs, 0), ffi::SQLITE_OK);
            Self { vfs, name, state }
        }
    }
    pub(crate) fn name(&self) -> String {
        self.name.to_str().unwrap().to_owned()
    }
}
impl Drop for Vfs {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(ffi::sqlite3_vfs_unregister(&mut *self.vfs), ffi::SQLITE_OK);
        }
        STATES
            .lock()
            .unwrap()
            .remove(&((&*self.vfs as *const _) as usize));
    }
}

unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    // SQLite allocates szOsFile bytes. Our aligned header precedes the original
    // VFS's file object, whose callbacks always receive its original address.
    unsafe {
        let state = Arc::clone(STATES.lock().unwrap().get(&(vfs as usize)).unwrap());
        let parent = state.parent as *mut ffi::sqlite3_vfs;
        let actual = file
            .cast::<u8>()
            .add(std::mem::size_of::<io::File>())
            .cast();
        let result = (*parent).xOpen.unwrap()(parent, name, actual, flags, out);
        if result == ffi::SQLITE_OK {
            file.cast::<io::File>().write(io::File {
                base: ffi::sqlite3_file {
                    pMethods: &io::METHODS,
                },
                actual,
                state: Arc::as_ptr(&state),
                database: flags & ffi::SQLITE_OPEN_MAIN_DB != 0,
            });
        }
        result
    }
}

unsafe extern "C" fn delete(vfs: *mut ffi::sqlite3_vfs, name: *const c_char, sync: c_int) -> c_int {
    unsafe {
        let state = Arc::clone(STATES.lock().unwrap().get(&(vfs as usize)).unwrap());
        let parent = state.parent as *mut ffi::sqlite3_vfs;
        state.before_unlink(name);
        if state.observe(Event::Delete(sync != 0)) {
            // Model deletion succeeding but its directory sync failing. The
            // transaction may be visible: callers must inspect, never re-effect.
            let result = (*parent).xDelete.unwrap()(parent, name, 0);
            if result == ffi::SQLITE_OK {
                ffi::SQLITE_IOERR_DIR_FSYNC
            } else {
                result
            }
        } else {
            let result = (*parent).xDelete.unwrap()(parent, name, sync);
            if result == ffi::SQLITE_OK && sync != 0 {
                state.directory_synced();
            }
            result
        }
    }
}
