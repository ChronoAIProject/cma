//! Test-only namespace-loss model driven by the actual SQLite VFS callbacks.
//! File syncs still reach the real VFS. Model the permitted power-loss outcome
//! in which the latest synced journal bytes survive but its unsynced unlink
//! does not. This is a deterministic control, not provider qualification.
use std::ffi::{c_char, CStr};
use std::io::Write;
use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::PathBuf;

use super::State;

#[derive(Default)]
pub(super) struct Namespace {
    journal: Option<(PathBuf, Vec<u8>)>,
}

impl State {
    pub(crate) fn model_namespace_loss(&self) {
        *self.namespace.lock().unwrap() = Some(Namespace::default());
    }

    pub(super) unsafe fn before_unlink(&self, name: *const c_char) {
        let mut state = self.namespace.lock().unwrap();
        let Some(state) = state.as_mut() else {
            return;
        };
        let name = unsafe { CStr::from_ptr(name) };
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(name.to_bytes()));
        assert!(path.to_string_lossy().ends_with("ledger.sqlite3-journal"));
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() <= 16 * 1024 * 1024);
        state.journal = Some((path, bytes));
    }

    pub(super) fn directory_synced(&self) {
        if let Some(state) = self.namespace.lock().unwrap().as_mut() {
            state.journal = None;
        }
    }

    /// Call only after every connection has closed. Restore the directory entry
    /// from the modeled durable namespace; leave already-synced DB bytes intact.
    pub(crate) fn lose_unsynced_namespace(&self) -> bool {
        let Some((path, bytes)) = self
            .namespace
            .lock()
            .unwrap()
            .take()
            .expect("namespace model enabled")
            .journal
        else {
            return false;
        };
        assert!(!path.exists());
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        true
    }
}
