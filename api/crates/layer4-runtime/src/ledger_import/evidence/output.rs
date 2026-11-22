use std::io::Write;

use super::super::inventory::Inventory;
use super::super::Budget;
use super::{error, records};
use crate::config::RuntimeConfig;
use crate::failure::Result;

struct Output {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode(
    config: &RuntimeConfig,
    inventory: &Inventory,
    budget: &mut Budget,
    output_limit: usize,
) -> Result<Vec<u8>> {
    let mut output = Output {
        bytes: Vec::new(),
        limit: output_limit,
    };
    let encoding = || error("ledger_evidence_output_budget");
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
        "generation": config.generation,
        "inventory_sha256": inventory.digest,
        "source_records": inventory.entries.len(),
        "source_directories": inventory.roots.len(),
    });
    serde_json::to_writer(&mut output, &header).map_err(|_| encoding())?;
    output.bytes.pop(); // Continue the object with the incrementally projected records.
    output
        .write_all(b",\"records\":[")
        .map_err(|_| encoding())?;
    for index in 0..inventory.entries.len() {
        budget.check()?;
        let record = records::project(config, inventory, index, budget)?;
        if index != 0 {
            output.write_all(b",").map_err(|_| encoding())?;
        }
        serde_json::to_writer(&mut output, &record).map_err(|_| encoding())?;
    }
    output.write_all(b"]}\n").map_err(|_| encoding())?;
    budget.check()?;
    Ok(output.bytes)
}
