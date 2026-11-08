//! Explicitly negotiated, content-free ledger counters on runtime status.
use crate::{AdapterError, Result, RuntimeStatus, RuntimeStatusRequest};
use serde::{Deserialize, Serialize};

pub const RUNTIME_LEDGER_STATUS_MEDIA_TYPE: &str =
    "application/vnd.cma.runtime-ledger-status.v1+json";

/// This envelope is returned only for the exact Accept value. A retained
/// runtime returns the baseline RuntimeStatus frame; decode reports unsupported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLedgerObservation {
    pub runtime: RuntimeStatus,
    pub ledger: RuntimeLedgerStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeLedgerStatus {
    /// No mutating V1 scanner runs for observation; counts are unknown.
    LegacyMigrationRequired,
    Indexed {
        counters: RuntimeLedgerCounters,
    },
}

/// History totals cover all retained generations. Normal admission and process
/// totals cover persistent/current-generation scopes. Reserved cleanup does not
/// consume the normal-outstanding allowance; zero is not proof of quiescence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLedgerCounters {
    pub revision: u64,
    pub operations: u64,
    pub terminal: u64,
    pub authority: u64,
    pub reserved_bytes: u64,
    pub normal_outstanding: u64,
    pub logical_processes: u64,
    pub allocated_bytes: u64,
    pub maximum_bytes: u64,
    pub normal_outstanding_limit: u64,
    pub transaction_headroom_bytes: u64,
    pub emergency_remaining_bytes: u64,
    pub emergency_limit_bytes: u64,
}

impl RuntimeLedgerObservation {
    pub fn validate_for(&self, request: &RuntimeStatusRequest) -> Result<()> {
        self.runtime.validate_for(request)?;
        if let RuntimeLedgerStatus::Indexed { counters: c } = &self.ledger {
            if c.terminal > c.operations
                || c.normal_outstanding > c.operations
                || c.normal_outstanding > c.normal_outstanding_limit
                || c.normal_outstanding_limit == 0
                || c.allocated_bytes > c.maximum_bytes
                || c.maximum_bytes == 0
                || c.emergency_remaining_bytes > c.emergency_limit_bytes
                || c.transaction_headroom_bytes > c.maximum_bytes
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8], request: &RuntimeStatusRequest) -> Result<Option<Self>> {
        if bytes.len() > 32768 {
            return Err(invalid());
        }
        if let Ok(observation) = serde_json::from_slice::<Self>(bytes) {
            observation.validate_for(request)?;
            return Ok(Some(observation));
        }
        match serde_json::from_slice::<crate::wire::ResponseFrame>(bytes) {
            Ok(crate::wire::ResponseFrame::RuntimeStatus(status)) => {
                status.validate_for(request)?;
                Ok(None)
            }
            _ => Err(invalid()),
        }
    }
}
fn invalid() -> AdapterError {
    AdapterError::Rejected("runtime ledger observation is invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> RuntimeStatusRequest {
        RuntimeStatusRequest {
            sandbox_id: "test".into(),
            generation: 1,
            home_identity: "a".repeat(43),
        }
    }
    fn runtime() -> RuntimeStatus {
        serde_json::from_value(serde_json::json!({"sandbox_id":"test", "generation":1, "home_identity":"a".repeat(43), "boot_id":"b".repeat(32),
            "installation_ready":true, "credential_ready":false, "restore_pending":false, "agent_processes":0})).unwrap()
    }
    #[test]
    fn legacy_and_unsupported_are_unknown_not_zero_or_loss() {
        let bytes =
            serde_json::to_vec(&crate::wire::ResponseFrame::RuntimeStatus(runtime())).unwrap();
        assert_eq!(
            RuntimeLedgerObservation::decode(&bytes, &request()).unwrap(),
            None
        );
        let observation = RuntimeLedgerObservation {
            runtime: runtime(),
            ledger: RuntimeLedgerStatus::LegacyMigrationRequired,
        };
        let bytes = serde_json::to_vec(&observation).unwrap();
        assert_eq!(
            RuntimeLedgerObservation::decode(&bytes, &request()).unwrap(),
            Some(observation)
        );
        let mut wrong = request();
        wrong.generation = 2;
        assert!(RuntimeLedgerObservation::decode(&bytes, &wrong).is_err());
        assert!(RuntimeLedgerObservation::decode(b"{}", &request()).is_err());
    }
    #[test]
    fn indexed_counts_are_bounded_and_identity_checked() {
        let mut observation = RuntimeLedgerObservation {
            runtime: runtime(),
            ledger: RuntimeLedgerStatus::Indexed {
                counters: RuntimeLedgerCounters {
                    revision: 3,
                    operations: 1,
                    terminal: 1,
                    authority: 1,
                    reserved_bytes: 0,
                    normal_outstanding: 0,
                    logical_processes: 0,
                    allocated_bytes: 4096,
                    maximum_bytes: 65536,
                    normal_outstanding_limit: 1024,
                    transaction_headroom_bytes: 4096,
                    emergency_remaining_bytes: 4096,
                    emergency_limit_bytes: 4096,
                },
            },
        };
        let bytes = serde_json::to_vec(&observation).unwrap();
        assert_eq!(
            RuntimeLedgerObservation::decode(&bytes, &request()).unwrap(),
            Some(observation.clone())
        );
        let RuntimeLedgerStatus::Indexed { counters } = &mut observation.ledger else {
            unreachable!()
        };
        counters.terminal = 2;
        assert!(observation.validate_for(&request()).is_err());
        assert!(RuntimeLedgerObservation::decode(&vec![b' '; 32769], &request()).is_err());
    }
}
