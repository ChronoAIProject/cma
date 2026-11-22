//! Validate imported artifact bytes; neither provenance nor restore is admitted.
mod records;
mod schema;
mod stream;

use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use super::super::io::{Directory, Identity};
use super::super::{Budget, Limits};
use crate::failure::{Result, RuntimeError};

fn limits() -> Limits {
    Limits {
        entries: 8,
        bytes: 2 * (super::OUTPUT_BYTES as u64 + super::SEAL_BYTES),
        metadata_bytes: 16 * 1024,
        deadline: Duration::from_secs(30),
    }
}

#[derive(Serialize)]
pub(in crate::ledger_import) struct Receipt {
    format: &'static str,
    artifact_integrity: &'static str,
    source_provenance: &'static str,
    producer_acknowledgement: &'static str,
    restore_admission: &'static str,
    semantic_reconciliation: &'static str,
    payload_sha256: String,
    payload_bytes: u64,
    sealed_sha256: String,
    source_records: usize,
}

pub(crate) async fn command(path: PathBuf, expected: String) -> std::result::Result<(), String> {
    let receipt = tokio::task::spawn_blocking(move || {
        verify(&path, &expected, &mut Budget::new(limits()), || Ok(()))
    })
    .await
    .map_err(|_| "ledger_evidence_verification_worker_failed".to_owned())?
    .map_err(|error| error.code)?;
    acknowledge(&receipt, &mut std::io::stdout().lock()).map_err(|error| error.code)
}

fn acknowledge(receipt: &Receipt, output: &mut impl Write) -> Result<()> {
    serde_json::to_writer(&mut *output, receipt).map_err(|_| acknowledgement_error())?;
    output
        .write_all(b"\n")
        .and_then(|_| output.flush())
        .map_err(|_| acknowledgement_error())
}

fn acknowledgement_error() -> RuntimeError {
    RuntimeError::rejected(
        "ledger_evidence_verification_acknowledgement_io",
        "evidence verification acknowledgement failed",
    )
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "ledger_evidence_verification_invalid",
        "evidence artifact verification did not complete",
    )
}

fn private_directory(path: &Path) -> Result<Directory> {
    let directory = Directory::open(path)?;
    let metadata = directory.0.metadata().map_err(|_| invalid())?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    Ok(directory)
}

fn names(directory: &Directory, budget: &mut Budget) -> Result<()> {
    let mut names = directory.names(budget)?;
    names.sort();
    if names != ["evidence.json", "sealed.json"] {
        return Err(invalid());
    }
    Ok(())
}

pub(in crate::ledger_import) fn verify(
    path: &Path,
    expected: &str,
    budget: &mut Budget,
    boundary: impl FnOnce() -> Result<()>,
) -> Result<Receipt> {
    if !schema::digest(expected) {
        return Err(invalid());
    }
    budget.check()?;
    let directory = private_directory(path)?;
    let identity = directory.identity()?;
    names(&directory, budget)?;
    let (seal, seal_hash, seal_identity) =
        stream::read::<schema::Seal>(&directory, "sealed.json", super::SEAL_BYTES, budget)?;
    if seal_hash != expected || !seal.valid() {
        return Err(invalid());
    }
    let (payload, payload_hash, payload_identity) = stream::read::<schema::Payload>(
        &directory,
        "evidence.json",
        super::OUTPUT_BYTES as u64,
        budget,
    )?;
    if !payload.valid()
        || payload_hash != seal.payload_sha256
        || payload_identity.bytes != seal.payload_bytes
        || payload.inventory_sha256 != seal.inventory_sha256
        || payload.source_records != seal.source_records
    {
        return Err(invalid());
    }
    boundary()?;
    recheck(
        &directory,
        "evidence.json",
        &payload_identity,
        &payload_hash,
        budget,
    )?;
    recheck(
        &directory,
        "sealed.json",
        &seal_identity,
        &seal_hash,
        budget,
    )?;
    names(&directory, budget)?;
    if private_directory(path)?.identity()? != identity || directory.identity()? != identity {
        return Err(invalid());
    }
    budget.check()?;
    Ok(Receipt {
        format: "runtime-ledger-v1-evidence-verification-v1",
        artifact_integrity: "VALIDATED",
        source_provenance: "UNVERIFIED",
        producer_acknowledgement: "UNVERIFIED",
        restore_admission: "HELD",
        semantic_reconciliation: "NOT_RUN",
        payload_sha256: payload_hash,
        payload_bytes: payload_identity.bytes,
        sealed_sha256: seal_hash,
        source_records: payload.source_records,
    })
}

fn recheck(
    directory: &Directory,
    name: &str,
    identity: &Identity,
    hash: &str,
    budget: &mut Budget,
) -> Result<()> {
    if directory.file_identity(name)? != *identity
        || directory.hash_file(name, identity.bytes, budget)? != hash
        || directory.file_identity(name)? != *identity
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_requires_write_and_flush() {
        struct Failing(bool);
        impl Write for Failing {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0 {
                    Err(std::io::ErrorKind::BrokenPipe.into())
                } else {
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let receipt = Receipt {
            format: "runtime-ledger-v1-evidence-verification-v1",
            artifact_integrity: "VALIDATED",
            source_provenance: "UNVERIFIED",
            producer_acknowledgement: "UNVERIFIED",
            restore_admission: "HELD",
            semantic_reconciliation: "NOT_RUN",
            payload_sha256: "a".repeat(64),
            payload_bytes: 1,
            sealed_sha256: "b".repeat(64),
            source_records: 1,
        };
        for write in [true, false] {
            assert_eq!(
                acknowledge(&receipt, &mut Failing(write)).unwrap_err().code,
                "ledger_evidence_verification_acknowledgement_io"
            );
        }
    }
}
