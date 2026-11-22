//! Explicit V1 observations remain held; native command correlation is unknown.
mod output;
mod records;
mod source;

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{changed, inventory, plan_io::Archive, validation, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

const OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const SEAL_BYTES: u64 = 4096;

#[derive(Serialize)]
pub(super) struct Receipt {
    format: &'static str,
    restore_admission: &'static str,
    semantic_reconciliation: &'static str,
    payload_sha256: String,
    payload_bytes: u64,
    sealed_sha256: String,
    source_records: usize,
}

pub(crate) async fn command(destination: PathBuf) -> std::result::Result<(), String> {
    let config = RuntimeConfig::from_environment()
        .map_err(|_| "ledger_evidence_configuration_invalid".to_owned())?;
    let receipt = tokio::task::spawn_blocking(move || {
        create(
            &config,
            &destination,
            &mut Budget::new(Limits::default()),
            OUTPUT_BYTES,
            source::readonly,
            |_| Ok(()),
        )
    })
    .await
    .map_err(|_| "ledger_evidence_worker_failed".to_owned())?
    .map_err(|failure| failure.code)?;
    acknowledge(&receipt, &mut std::io::stdout().lock()).map_err(|failure| failure.code)
}

fn acknowledge(receipt: &Receipt, sink: &mut impl Write) -> Result<()> {
    serde_json::to_writer(&mut *sink, receipt)
        .map_err(|_| error("ledger_evidence_acknowledgement_io"))?;
    sink.write_all(b"\n")
        .and_then(|_| sink.flush())
        .map_err(|_| error("ledger_evidence_acknowledgement_io"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Boundary {
    Validated,
    Projected,
    PayloadStored,
}

fn validate(
    config: &RuntimeConfig,
    budget: &mut Budget,
    mount_check: fn(&File) -> Result<()>,
) -> Result<inventory::Inventory> {
    source::preflight(config, budget, mount_check)?;
    let inventory = inventory::scan_with_budget(config, budget)?;
    source::verify(&inventory, budget, mount_check)?;
    // Keep strict history checks, then release their index before projection.
    drop(validation::analyze_with_budget(config, &inventory, budget)?);
    Ok(inventory)
}

fn unchanged(
    config: &RuntimeConfig,
    inventory: &inventory::Inventory,
    budget: &mut Budget,
    mount_check: fn(&File) -> Result<()>,
) -> Result<()> {
    source::preflight(config, budget, mount_check)?;
    if inventory::scan_with_budget(config, budget)?.digest != inventory.digest {
        return Err(changed());
    }
    source::verify(inventory, budget, mount_check)?;
    budget.check()
}

pub(super) fn create(
    config: &RuntimeConfig,
    destination: &Path,
    budget: &mut Budget,
    output_limit: usize,
    mount_check: fn(&File) -> Result<()>,
    mut boundary: impl FnMut(Boundary) -> Result<()>,
) -> Result<Receipt> {
    let inventory = validate(config, budget, mount_check)?;
    boundary(Boundary::Validated)?;
    let archive = Archive::create(config, destination)?;
    let mut encoded = None;
    archive.write("evidence.json", |file| {
        encoded = Some(output::encode(
            config,
            &inventory,
            budget,
            output_limit,
            file,
        )?);
        boundary(Boundary::Projected)?;
        unchanged(config, &inventory, budget, mount_check)
    })?;
    boundary(Boundary::PayloadStored)?;
    unchanged(config, &inventory, budget, mount_check)?;
    let encoded = encoded.ok_or_else(|| error("ledger_evidence_output_io"))?;
    let sealed_sha256 = archive.json_bounded(
        "sealed.json",
        &serde_json::json!({
            "format": "runtime-ledger-v1-evidence-seal-v1",
            "restore_admission": "HELD",
            "semantic_reconciliation": "NOT_RUN",
            "payload_sha256": encoded.sha256,
            "payload_bytes": encoded.bytes,
            "inventory_sha256": inventory.digest,
            "source_records": inventory.entries.len(),
        }),
        SEAL_BYTES,
    )?;
    Ok(Receipt {
        format: "runtime-ledger-v1-evidence-receipt-v1",
        restore_admission: "HELD",
        semantic_reconciliation: "NOT_RUN",
        payload_sha256: encoded.sha256,
        payload_bytes: encoded.bytes,
        sealed_sha256,
        source_records: inventory.entries.len(),
    })
}

#[cfg(test)]
pub(super) fn collect(
    config: &RuntimeConfig,
    budget: &mut Budget,
    output_limit: usize,
    mount_check: fn(&File) -> Result<()>,
    boundary: impl FnMut(Boundary) -> Result<()>,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    collect_to(
        config,
        budget,
        output_limit,
        mount_check,
        boundary,
        &mut bytes,
    )?;
    Ok(bytes)
}

#[cfg(test)]
pub(super) fn collect_to(
    config: &RuntimeConfig,
    budget: &mut Budget,
    output_limit: usize,
    mount_check: fn(&File) -> Result<()>,
    mut boundary: impl FnMut(Boundary) -> Result<()>,
    sink: &mut impl Write,
) -> Result<()> {
    let inventory = validate(config, budget, mount_check)?;
    boundary(Boundary::Validated)?;
    output::encode(config, &inventory, budget, output_limit, sink)?;
    boundary(Boundary::Projected)?;
    unchanged(config, &inventory, budget, mount_check)
}

fn error(code: &'static str) -> RuntimeError {
    RuntimeError::rejected(code, "V1 ledger evidence collection did not complete")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledgement_write_and_flush_failures_are_not_success() {
        struct Sink {
            fail_write: bool,
            bytes: Vec<u8>,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.fail_write {
                    return Err(std::io::ErrorKind::BrokenPipe.into());
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let receipt = Receipt {
            format: "runtime-ledger-v1-evidence-receipt-v1",
            restore_admission: "HELD",
            semantic_reconciliation: "NOT_RUN",
            payload_sha256: "a".repeat(64),
            payload_bytes: 0,
            sealed_sha256: "b".repeat(64),
            source_records: 0,
        };
        for fail_write in [true, false] {
            let mut sink = Sink {
                fail_write,
                bytes: Vec::new(),
            };
            assert_eq!(
                acknowledge(&receipt, &mut sink).unwrap_err().code,
                "ledger_evidence_acknowledgement_io"
            );
            assert_eq!(sink.bytes.is_empty(), fail_write);
        }
    }
}
