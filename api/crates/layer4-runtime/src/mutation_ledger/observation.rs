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
                normal_outstanding: unsigned(s.pending)?,
                logical_processes: unsigned(s.processes)?,
                allocated_bytes: unsigned(s.allocated_bytes)?,
                maximum_bytes: unsigned(s.maximum_bytes)?,
                normal_outstanding_limit: unsigned(budget::OUTSTANDING)?,
                transaction_headroom_bytes: unsigned(budget::HEADROOM)?,
                emergency_remaining_bytes: s.emergency_remaining_bytes,
                emergency_limit_bytes: crate::ledger_storage::reserve::BYTES,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutation_ledger::tests::{add, remove, Fixture};
    #[tokio::test]
    async fn reserved_cleanup_is_not_misreported_as_normal_admission_or_quiescence() {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        ledger.admit(1, vec![add(0)]).await.unwrap();
        let record = ledger.begin(add(0)).await.unwrap().record;
        ledger
            .finish(
                record
                    .failed(
                        &crate::failure::RuntimeError::rejected("synthetic", "synthetic failure"),
                        false,
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        ledger.admit(2, vec![remove()]).await.unwrap();
        ledger.begin(remove()).await.unwrap();
        let RuntimeLedgerStatus::Indexed { counters } = ledger.observation().await.unwrap() else {
            unreachable!()
        };
        assert_eq!(counters.normal_outstanding, 0);
        assert_eq!(counters.operations, 2);
        assert_eq!(counters.terminal, 1);
        assert!(counters.reserved_bytes > 0);
        assert_eq!(
            ledger.started_page(0, String::new()).await.unwrap().len(),
            1
        );
        ledger.close().await.unwrap();
    }
}
