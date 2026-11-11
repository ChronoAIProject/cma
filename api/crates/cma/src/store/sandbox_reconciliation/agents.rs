use crate::store::agent_log_telemetry::ScanCaller;
use std::collections::BTreeMap;

use layer2_core::sandbox::store::{SandboxRuntimeLossAgentMutation, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentLogEntry};
use layer2_core::SandboxId;
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use super::super::agent_log::scan_complete_log_with_session;
use super::super::agent_log_integrity::{prepare_append, AppendCapacity, LogSummary};
use super::super::agents::agent_replacement_fields;
use super::super::lifecycle_execution::load_sandbox_agents;
use super::super::sandbox_mongo::{
    is_duplicate_key, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::transaction::ApplyError;

pub(super) async fn load_agents(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<Vec<(Agent, LogSummary)>, ApplyError> {
    let agents = load_sandbox_agents(store, id, session).await?;
    let mut rows = Vec::with_capacity(agents.len());
    for agent in agents {
        let mut entries = Vec::new();
        let complete = scan_complete_log_with_session(
            ScanCaller::RuntimeReconciliation,
            store,
            &agent.id,
            agent.conversation.last_seq,
            session,
            |entry| {
                entries.push(entry);
                Ok(())
            },
        )
        .await?;
        let semantic =
            crate::store::agent_log_integrity::audit_checkpoint(store, &agent, &entries, session)
                .await?;
        rows.push((agent, complete.with_semantic(semantic)));
    }
    Ok(rows)
}

pub(super) async fn publish_agents(
    store: &MongoSandboxStore,
    rows: Vec<(Agent, LogSummary)>,
    changes: &[SandboxRuntimeLossAgentMutation],
    at: &str,
    session: &mut ClientSession,
) -> Result<(Vec<Agent>, Vec<AgentLogEntry>), ApplyError> {
    let changes = changes
        .iter()
        .map(|change| (&change.agent.id, change))
        .collect::<BTreeMap<_, _>>();
    let mut agents = Vec::with_capacity(rows.len());
    let mut logs = Vec::new();
    let mut encoded_logs = Vec::new();
    for (current, complete) in rows {
        let filter = agent_filter(&current)?;
        let update = match changes.get(&current.id) {
            Some(change) => {
                let batch = materialize_logs(change, at)?;

                let mut fields =
                    agent_replacement_fields(&change.agent).map_err(ApplyError::Domain)?;
                let semantic = complete.semantic().cloned().ok_or(ApplyError::Domain(
                    SandboxStoreError::Corrupt(
                        "runtime reconciliation scan did not produce semantic checkpoint".into(),
                    ),
                ))?;
                let integrity = prepare_append(
                    &current,
                    &complete,
                    &batch,
                    change.agent.revision,
                    semantic
                        .after_append(&batch, &change.agent)
                        .map_err(ApplyError::Domain)?,
                    AppendCapacity::StopOrFailure,
                )
                .map_err(ApplyError::Domain)?;
                encoded_logs.extend(integrity.documents);
                fields.insert("agent_log_integrity_v1", integrity.integrity);
                fields.insert(
                    "last_seq",
                    signed(
                        change.agent.conversation.last_seq,
                        "runtime-loss Agent log sequence",
                    )
                    .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
                );
                agents.push(change.agent.clone());
                logs.extend(batch);
                doc! { "$set": fields }
            }
            None => {
                // Terminal Agent evidence participates in the same snapshot decision.
                agents.push(current);
                doc! { "$inc": { "sandbox_recovery_fence": 1_i64 } }
            }
        };
        let advanced = store
            .agents
            .update_one(filter, update)
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if advanced.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    for entry in &encoded_logs {
        store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
            .insert_one(entry)
            .session(&mut *session)
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    ApplyError::Domain(SandboxStoreError::SequenceConflict)
                } else {
                    ApplyError::Driver(error)
                }
            })?;
    }
    Ok((agents, logs))
}

fn agent_filter(agent: &Agent) -> Result<Document, ApplyError> {
    Ok(doc! {
        "_id": agent.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "sandbox_id": agent.sandbox_id.as_str(),
        "owner": agent.owner.as_str(),
        "revision": signed(agent.revision, "runtime-loss Agent revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        "last_seq": signed(agent.conversation.last_seq, "runtime-loss Agent log sequence")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?,
    })
}

fn materialize_logs(
    change: &SandboxRuntimeLossAgentMutation,
    at: &str,
) -> Result<Vec<AgentLogEntry>, ApplyError> {
    change
        .logs
        .iter()
        .enumerate()
        .map(|(offset, kind)| {
            let offset = u64::try_from(offset)
                .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
            let seq = change
                .expected_last_seq
                .checked_add(offset)
                .and_then(|seq| seq.checked_add(1))
                .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
            Ok(AgentLogEntry {
                agent_id: change.agent.id.clone(),
                seq,
                appended_at: at.into(),
                kind: kind.clone(),
            })
        })
        .collect()
}
