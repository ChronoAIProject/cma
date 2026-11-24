//! A discovered completed carrier is a routing candidate, never an IO grant.
use super::{discovery, receipts, transactions, RecoveryService};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::{read_format, transaction_scope::TransactionScope},
    agents::read_agent,
};
use layer2_core::{
    sandbox::{
        agent_capacity_recovery::{
            ports::RecoveryError,
            state::{RecoveryPhase, RecoveryRequest},
        },
        store::SandboxStoreError,
    },
    AgentId,
};
use mongodb::bson::doc;

pub(crate) enum RestartDecision {
    /// Positively validated legacy envelope; the ordinary route still performs
    /// its own revision, intent and lease checks before scheduling anything.
    Legacy,
    Completed(RecoveryRequest),
    /// Segmented absence or another phase does not prove that ordinary
    /// reconnection is safe. Later progress needs positive current evidence.
    Deferred,
}

impl RecoveryService {
    pub(crate) async fn restart_decision(
        &self,
        id: &AgentId,
    ) -> Result<RestartDecision, RecoveryError> {
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(|_| RecoveryError::Unavailable)?;
        let mut scope = TransactionScope::start(&mut session)
            .await
            .map_err(|_| RecoveryError::Unavailable)?;
        let result = async {
            let row = scope
                .find_one(&self.store.agents, doc! {"_id": id.as_str()})
                .await
                .map_err(TransactionLogError::Driver)?
                .ok_or(SandboxStoreError::NotFound)?;
            let agent = read_agent(&row)?;
            if read_format(&row)?.is_none() {
                return Ok(RestartDecision::Legacy);
            }
            // Discovery is capped at32 requests/128KiB. Every candidate is then
            // re-read and bound to its immutable publication in this snapshot.
            let requests = discovery::for_agent(&self.store, id).await.map_err(|_| {
                SandboxStoreError::Unavailable("recovery restart discovery refused".into())
            })?;
            for request in requests {
                let Some(stored) =
                    receipts::read(&self.store, &agent, &request, &mut scope).await?
                else {
                    return Ok(RestartDecision::Deferred);
                };
                if stored.work.phase == RecoveryPhase::Completed {
                    receipts::require_current_result(&self.store, &agent, &stored, &mut scope)
                        .await?;
                    return Ok(RestartDecision::Completed(request));
                }
            }
            Ok::<_, TransactionLogError>(RestartDecision::Deferred)
        }
        .await;
        let _ = scope.release().abort_guarded().await;
        result.map_err(transactions::error)
    }
}
