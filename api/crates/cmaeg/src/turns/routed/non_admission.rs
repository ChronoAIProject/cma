//! Retained diagnostic evidence, separate from the rollback-readable target state.
use super::{storage, ExecutionError, Journal, Lease, Phase, TargetState};
use serde::{Deserialize, Serialize};

const KIND: &str = "routed_non_admission_v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    kind: String,
    target_id: String,
    root: String,
    request_hash: String,
    body_hash: String,
    attempt_epoch: u64,
    code: String,
}
fn id(state: &TargetState) -> String {
    storage::identifier(&(KIND, &state.id))
}
fn valid(code: &str) -> bool {
    matches!(
        code,
        "conversation_not_ready" | "verified_sender_context_unavailable"
    )
}
impl Receipt {
    fn validate(&self, state: &TargetState) -> Result<(), ExecutionError> {
        if self.kind != KIND
            || self.target_id != state.id
            || self.root != state.root
            || self.request_hash != state.request_hash
            || state
                .body
                .as_ref()
                .is_none_or(|body| body.hash != self.body_hash)
            || self.attempt_epoch == 0
            || !valid(&self.code)
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }
}
impl Journal {
    pub(super) async fn record_non_admission(
        &self,
        lease: &Lease,
        code: &str,
    ) -> Result<(), ExecutionError> {
        self.guard(lease).await?;
        let state = &lease.state;
        if state.phase != Phase::Starting || !valid(code) {
            return Err(ExecutionError::Invalid);
        }
        let key = id(state);
        let current = self.read::<Receipt>(&key).await?;
        if let Some((receipt, _)) = &current {
            receipt.validate(state)?;
            if receipt.attempt_epoch > state.ownership.epoch {
                return Err(ExecutionError::LostLease);
            }
        }
        let receipt = Receipt {
            kind: KIND.into(),
            target_id: state.id.clone(),
            root: state.root.clone(),
            request_hash: state.request_hash.clone(),
            body_hash: state
                .body
                .as_ref()
                .ok_or(ExecutionError::Invalid)?
                .hash
                .clone(),
            attempt_epoch: state.ownership.epoch,
            code: code.into(),
        };
        self.write(
            &key,
            &state.owner,
            state.retained_until,
            &receipt,
            current.map(|(_, version)| version),
        )
        .await?;
        self.guard(lease).await
    }

    pub(super) async fn last_non_admission(
        &self,
        state: &TargetState,
    ) -> Result<Option<String>, ExecutionError> {
        if state.body.is_none() {
            return Ok(None);
        }
        let Some((receipt, _)) = self.read::<Receipt>(&id(state)).await? else {
            return Ok(None);
        };
        receipt.validate(state)?;
        Ok(Some(receipt.code))
    }
}
