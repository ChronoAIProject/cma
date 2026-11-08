use super::{budget, invalid, Ledger};
use crate::failure::Result;
use layer3_adapter::{RuntimeLedgerCounters, RuntimeLedgerStatus};

impl Ledger {
    pub(crate) async fn observation(&self) -> Result<RuntimeLedgerStatus> {
        let s = self.snapshot().await?;
        let unsigned = |n| u64::try_from(n).map_err(|_| invalid());
        Ok(RuntimeLedgerStatus::Indexed {
            counters: RuntimeLedgerCounters {
                revision: unsigned(s.revision)?,
                operations: unsigned(s.operations)?,
                terminal: unsigned(s.terminal)?,
                authority: unsigned(s.authority)?,
                reserved_bytes: unsigned(s.reserved_bytes)?,
                outstanding: unsigned(s.pending)?,
                logical_processes: unsigned(s.processes)?,
                allocated_bytes: unsigned(s.allocated_bytes)?,
                maximum_bytes: unsigned(s.maximum_bytes)?,
                outstanding_limit: unsigned(budget::OUTSTANDING)?,
                transaction_headroom_bytes: unsigned(budget::HEADROOM)?,
                emergency_remaining_bytes: s.emergency_remaining_bytes,
                emergency_limit_bytes: crate::ledger_storage::reserve::BYTES,
            },
        })
    }
}
