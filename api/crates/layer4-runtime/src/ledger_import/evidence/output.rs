use std::io::Write;

use sha2::{Digest, Sha256};

use super::super::inventory::Inventory;
use super::super::Budget;
use super::{error, records};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

pub(super) struct Encoded {
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

struct Output<'a, W> {
    sink: &'a mut W,
    bytes: u64,
    limit: u64,
    digest: Sha256,
}

impl<W: Write> Write for Output<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        let length = self.sink.write(bytes)?;
        self.bytes += length as u64;
        self.digest.update(&bytes[..length]);
        Ok(length)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.sink.flush()
    }
}

fn encoding(error: serde_json::Error) -> RuntimeError {
    io_error(error.io_error_kind())
}

fn io_error(kind: Option<std::io::ErrorKind>) -> RuntimeError {
    error(if kind == Some(std::io::ErrorKind::FileTooLarge) {
        "ledger_evidence_output_budget"
    } else {
        "ledger_evidence_output_io"
    })
}

pub(super) fn encode(
    config: &RuntimeConfig,
    inventory: &Inventory,
    budget: &mut Budget,
    output_limit: usize,
    sink: &mut impl Write,
) -> Result<Encoded> {
    let mut output = Output {
        sink,
        bytes: 0,
        limit: output_limit as u64,
        digest: Sha256::new(),
    };
    let header = serde_json::json!({
        "format": "runtime-ledger-v1-evidence-v1",
        "scope": "v1_mutation_and_auxiliary_metadata_only",
        "enumeration_complete": true,
        "restore_admission": "HELD",
        "semantic_reconciliation": "NOT_RUN",
        "native_command_correlation": "UNKNOWN",
        "native_session_inventory": "NOT_ASSESSED",
        "runtime_output_boundary": "UNKNOWN",
        "retirement_inventory": "UNSUPPORTED",
        "full_home_inventory": "NOT_ASSESSED",
        "provider_identity": "UNVERIFIED",
        "writer_exclusion": "UNVERIFIED",
        "external_immutability": "UNVERIFIED",
        "mutation_histories": "VALIDATED",
        "auxiliary_recovery": "UNQUALIFIED",
        "sandbox_id": config.sandbox_id,
        "home_identity": config.home_identity,
        "generation": config.generation.to_string(),
        "inventory_sha256": inventory.digest,
        "source_records": inventory.entries.len(),
        "source_directories": inventory.roots.len(),
    });
    let fields = header.as_object().ok_or_else(super::super::invalid)?;
    output
        .write_all(b"{")
        .map_err(|e| io_error(Some(e.kind())))?;
    for (key, value) in fields {
        serde_json::to_writer(&mut output, key).map_err(encoding)?;
        output
            .write_all(b":")
            .map_err(|e| io_error(Some(e.kind())))?;
        serde_json::to_writer(&mut output, value).map_err(encoding)?;
        output
            .write_all(b",")
            .map_err(|e| io_error(Some(e.kind())))?;
    }
    output
        .write_all(b"\"records\":[")
        .map_err(|e| io_error(Some(e.kind())))?;
    for index in 0..inventory.entries.len() {
        budget.check()?;
        let record = records::project(config, inventory, index, budget)?;
        if index != 0 {
            output
                .write_all(b",")
                .map_err(|e| io_error(Some(e.kind())))?;
        }
        serde_json::to_writer(&mut output, &record).map_err(encoding)?;
    }
    output
        .write_all(b"]}\n")
        .map_err(|e| io_error(Some(e.kind())))?;
    output.flush().map_err(|e| io_error(Some(e.kind())))?;
    budget.check()?;
    Ok(Encoded {
        bytes: output.bytes,
        sha256: crate::command::hex(&output.digest.finalize()),
    })
}
