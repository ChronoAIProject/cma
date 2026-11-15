use super::cleanup::{ExitObservation, Failure};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(super) struct PrivateDirectory {
    path: PathBuf,
    spawned: bool,
    exit_confirmed: bool,
    removed: bool,
    receipt_written: bool,
}

impl PrivateDirectory {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cma-key-provider-contract-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self {
            path,
            spawned: false,
            exit_confirmed: false,
            removed: false,
            receipt_written: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn spawned(&mut self) {
        self.spawned = true;
    }

    fn write_receipt(
        &mut self,
        observed: &ExitObservation,
        closed: Option<Value>,
    ) -> io::Result<()> {
        let receipt = json!({
            "schema": "provider-test-cleanup-v1", "child_spawned": self.spawned,
            "exit_confirmed": self.exit_confirmed, "directory_removed": false,
            "kill_requested": observed.kill_requested, "failures": observed.failures,
            "closed_receipt_summary": closed,
        });
        // create_new prevents overwriting earlier custody evidence. Only fixed
        // statuses and allowlisted close fields reach this private receipt.
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(self.path.join("cleanup-receipt.json"))?;
        file.write_all(serde_json::to_string(&receipt)?.as_bytes())?;
        file.sync_all()?;
        self.receipt_written = true;
        eprintln!("provider fixture cleanup observation: {receipt}");
        Ok(())
    }

    pub fn finish(
        &mut self,
        mut observed: ExitObservation,
        closed: Option<Value>,
    ) -> Result<(), Failure> {
        self.exit_confirmed = observed.exit_confirmed;
        if !self.exit_confirmed && observed.failures.is_empty() {
            observed.failures.push(Failure::Interrupted);
        }
        if closed.is_none() && observed.failures.is_empty() {
            observed.failures.push(Failure::ReceiptInvalid);
        }
        match std::fs::symlink_metadata(self.path.join("provider.lock")) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => observed.failures.push(Failure::LockRemains),
            Err(_) => observed.failures.push(Failure::StateIo),
        }
        self.write_receipt(&observed, closed)
            .map_err(|_| Failure::StateIo)?;
        if let Some(failure) = observed.failures.first() {
            return Err(*failure);
        }
        std::fs::remove_dir_all(&self.path).map_err(|_| Failure::StateIo)?;
        self.removed = true;
        eprintln!("provider fixture cleanup complete: exit_confirmed=true directory_removed=true");
        Ok(())
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        if !self.spawned {
            // Definite pre-spawn failure has no child that could still write.
            if std::fs::remove_dir_all(&self.path).is_err() {
                eprintln!(
                    "provider pre-spawn directory cleanup failed: {}",
                    self.path.display()
                );
            }
            return;
        }
        if !self.receipt_written {
            let observed = ExitObservation {
                exit_confirmed: self.exit_confirmed,
                failures: vec![Failure::Interrupted],
                ..Default::default()
            };
            if self.write_receipt(&observed, None).is_err() {
                eprintln!("provider incomplete-cleanup receipt write failed");
            }
        }
        eprintln!("provider fixture state retained: {}", self.path.display());
    }
}
