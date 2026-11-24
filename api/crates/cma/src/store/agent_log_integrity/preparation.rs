//! Serial, bounded preparation before an explicit cohort can use trusted reads.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, start_transaction, CommitOutcome, DOCUMENT_SCHEMA_VERSION,
};
use crate::store::work_claim_authority::{without_ledger_maintenance, LEDGER_MAINTENANCE_FIELD};
use crate::store::work_schema;
use std::time::Duration;

impl MongoSandboxStore {
    pub(crate) async fn prepare_bounded_admission(
        mut self,
        ids: &[AgentId],
    ) -> Result<Self, String> {
        if ids.len() > 16 {
            return Err("Agent log admission cohort exceeds preparation budget".into());
        }
        // No tasks are detached and no user request performs cohort preparation.
        // A timed-out session is dropped/aborted by the driver. An ambiguous
        // metadata-only commit can be verified safely on the next startup.
        let result = tokio::time::timeout(Duration::from_secs(60), async {
            for id in ids {
                let prepared = tokio::time::timeout(Duration::from_secs(10), prepare(&self, id))
                    .await
                    .map_err(|_| "Agent log preparation timed out")??;
                if prepared {
                    self.bounded_admission_agents.insert(id.clone());
                }
            }
            Ok::<_, String>(())
        })
        .await
        .map_err(|_| "Agent log cohort preparation exceeded 60 seconds")?;
        result?;
        Ok(self)
    }
}

/// Returns false when the parent is held and no preparation was committed.
pub(super) async fn prepare(store: &MongoSandboxStore, id: &AgentId) -> Result<bool, String> {
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
    let parent = store
        .sandboxes
        .find_one(parent_filter(&agent))
        .projection(doc! {"_id": 1, LEDGER_MAINTENANCE_FIELD: 1})
        .session(&mut session)
        .await
        .map_err(|_| "preparation Sandbox lookup unavailable")?
        .ok_or("preparation Sandbox missing or invalid")?;
    if parent.contains_key(LEDGER_MAINTENANCE_FIELD) {
        let _ = session.abort_transaction().await;
        return Ok(false);
    }
    let mut entries = Vec::new();
    let result = scan_complete_log_with_policy(
        ScanCaller::RecoveryProof,
        store,
        id,
        agent.conversation.last_seq,
        &mut session,
        IntegrityFailurePolicy::FencedPreparation,
        |entry| {
            entries.push(entry);
            Ok(())
        },
    )
    .await;
    let summary = result.map_err(|_| "preparation history validation failed")?;
    let semantic = match semantic::audit_checkpoint_with_policy(
        store,
        &agent,
        &entries,
        &mut session,
        IntegrityFailurePolicy::FencedPreparation,
    )
    .await
    {
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
    fence_parent(store, &agent, &mut session)
        .await
        .map_err(|_| "preparation Sandbox fence unavailable")?;
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(true),
        _ => Err("preparation commit unconfirmed; cohort remains disabled".into()),
    }
}

fn parent_filter(agent: &Agent) -> Document {
    work_schema::query(doc! {
        "_id": agent.sandbox_id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": agent.owner.as_str(),
        "record.owner": agent.owner.as_str(),
    })
}

pub(super) async fn fence_parent(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<(), TransactionLogError> {
    // A write to the parent serializes this metadata commit with hold admission.
    let fenced = store
        .sandboxes
        .update_one(
            without_ledger_maintenance(parent_filter(agent)),
            doc! {"$inc": {"agent_log_preparation_fence": 1_i64}},
        )
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if fenced.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(())
}
