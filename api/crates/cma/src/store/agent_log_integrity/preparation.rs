//! Serial, bounded preparation before an explicit cohort can use trusted reads.
use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use std::time::Duration;

impl MongoSandboxStore {
    pub(crate) async fn prepare_bounded_admission(
        mut self,
        ids: &[AgentId],
    ) -> Result<Self, String> {
        let prepared = prepare_cohort(ids, |id| prepare(&self, id)).await?;
        self.bounded_admission_agents.extend(prepared);
        Ok(self)
    }
}

// Keep the serial startup budget independent of Mongo proof details. A failed
// cohort never returns a store to composition, even after partial preparation.
async fn prepare_cohort<'a, F, Fut>(
    ids: &'a [AgentId],
    mut operation: F,
) -> Result<Vec<AgentId>, String>
where
    F: FnMut(&'a AgentId) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    if ids.len() > 16 {
        return Err("Agent log admission cohort exceeds preparation budget".into());
    }
    // No tasks are detached. Ambiguous metadata-only commits can be verified
    // safely on the next startup; no unconfirmed cohort becomes ready.
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut prepared = Vec::new();
        for id in ids {
            tokio::time::timeout(Duration::from_secs(10), operation(id))
                .await
                .map_err(|_| "Agent log preparation timed out")??;
            prepared.push(id.clone());
        }
        Ok(prepared)
    })
    .await
    .map_err(|_| "Agent log cohort preparation exceeded 60 seconds")?
}

#[cfg(test)]
mod tests;

pub(super) async fn prepare(store: &MongoSandboxStore, id: &AgentId) -> Result<(), String> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| "preparation session unavailable")?;
    start_transaction(&mut session)
        .await
        .map_err(|_| "preparation transaction unavailable")?;
    let row = store
        .agents
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut session)
        .await
        .map_err(|_| "preparation lookup unavailable")?
        .ok_or("preparation Agent missing")?;
    let agent = crate::store::agents::read_agent(&row).map_err(|_| "preparation Agent invalid")?;
    let mut entries = Vec::new();
    let result = scan_complete_log_with_session(
        ScanCaller::RecoveryProof,
        store,
        id,
        agent.conversation.last_seq,
        &mut session,
        |entry| {
            entries.push(entry);
            Ok(())
        },
    )
    .await;
    let summary = result.map_err(|_| "preparation history validation failed")?;
    let semantic = match audit_checkpoint(store, &agent, &entries, &mut session).await {
        Ok(semantic) if semantic.valid => semantic,
        _ => {
            let _ = session.abort_transaction().await;
            return Err("preparation semantic checkpoint unavailable".into());
        }
    };
    let integrity = integrity_document(&agent, &summary, agent.revision, semantic)
        .map_err(|_| "preparation metadata invalid")?;
    let result = store
        .agents
        .update_one(
            doc! {
                "_id": id.as_str(), "revision": agent.revision as i64,
                "last_seq": agent.conversation.last_seq as i64,
                "record.conversation.last_seq": agent.conversation.last_seq as i64,
                QUARANTINE_FIELD: { "$exists": false },
            },
            doc! { "$set": { INTEGRITY_FIELD: integrity } },
        )
        .session(&mut session)
        .await
        .map_err(|_| "preparation CAS unavailable")?;
    if result.matched_count != 1 {
        let _ = session.abort_transaction().await;
        return Err("preparation conflicted with an Agent writer".into());
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(()),
        _ => Err("preparation commit unconfirmed; cohort remains disabled".into()),
    }
}
