use crate::store::agent_log_telemetry::ScanCaller;
use std::collections::BTreeMap;

use layer2_core::sandbox::store::{
    SandboxRuntimeLossAgentMutation, SandboxRuntimeLossRequest, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentLogEntry};
use layer2_core::SandboxId;
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::agent_capacity_recovery::source::{capture_segmented, fence_filter};
use super::super::agent_log::scan_complete_log_with_session;
use super::super::agent_log_integrity::{prepare_append, AppendCapacity, LogSummary};
use super::super::agent_log_segments::{
    capture::CapturedLog,
    publication::{self, PublicationIdentity},
    writer_commit::{self, AggregateUpdate, PublicationInput},
};
use super::super::agents::agent_replacement_fields;
use super::super::lifecycle_execution::load_sandbox_agents;
use super::super::sandbox_mongo::{is_duplicate_key, signed, MongoSandboxStore};
use super::transaction::ApplyError;

pub(super) enum Source {
    Legacy(Box<LogSummary>),
    Segmented(Box<CapturedLog>),
}

impl Source {
    pub(super) fn through(&self) -> u64 {
        match self {
            Self::Legacy(summary) => summary.entry_count(),
            Self::Segmented(captured) => captured.through(),
        }
    }
}

pub(super) async fn load_agents(
    store: &MongoSandboxStore,
    id: &SandboxId,
    session: &mut ClientSession,
) -> Result<Vec<(Agent, Source)>, ApplyError> {
    let agents = load_sandbox_agents(store, id, session).await?;
    let mut rows = Vec::with_capacity(agents.len());
    for agent in agents {
        if let Some(source) = capture_segmented(store, &agent, session).await? {
            source
                .projection()
                .validate_aggregate(&agent)
                .map_err(|detail| ApplyError::Domain(SandboxStoreError::Corrupt(detail)))?;
            rows.push((agent, Source::Segmented(Box::new(source))));
            continue;
        }
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
        rows.push((
            agent,
            Source::Legacy(Box::new(complete.with_semantic(semantic))),
        ));
    }
    Ok(rows)
}

pub(super) async fn publish_agents(
    store: &MongoSandboxStore,
    rows: Vec<(Agent, Source)>,
    changes: &[SandboxRuntimeLossAgentMutation],
    request: &SandboxRuntimeLossRequest,
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
        let filter = fence_filter(store, &current, session).await?;
        let complete = match complete {
            Source::Legacy(summary) => summary,
            Source::Segmented(source) => {
                if let Some(change) = changes.get(&current.id) {
                    let batch = materialize_logs(change, &request.observed_at)?;
                    let operation = PublicationIdentity {
                        key: format!(
                            "runtime-loss:{}:{}:{}",
                            request.sandbox_id, request.expected_revision, current.id
                        ),
                        fingerprint: publication::request_digest(&(
                            "runtime-loss/v1",
                            &request.sandbox_id,
                            request.expected_revision,
                            &request.expected_runtime,
                            request.retain_runtime,
                            request.interruption_cause,
                            &request.observed_at,
                            &current.id,
                        ))
                        .map_err(ApplyError::Domain)?,
                        appended_at: request.observed_at.clone(),
                    };
                    let prepared = writer_commit::prepare(
                        store,
                        session,
                        &source,
                        PublicationInput {
                            operation: &operation,
                            current: &current,
                            next: &change.agent,
                            logs: &batch,
                            capacity: AppendCapacity::StopOrFailure,
                            result_digest: publication::result_digest(&(&change.agent, &batch))
                                .map_err(ApplyError::Domain)?,
                        },
                        None,
                        None,
                    )
                    .await?;
                    writer_commit::persist(
                        store,
                        session,
                        prepared,
                        AggregateUpdate {
                            filter,
                            fields: agent_replacement_fields(&change.agent)
                                .map_err(ApplyError::Domain)?,
                        },
                    )
                    .await?;
                    agents.push(change.agent.clone());
                    logs.extend(batch);
                } else {
                    let result = store
                        .agents
                        .update_one(filter, doc! {"$inc": {"sandbox_recovery_fence": 1_i64}})
                        .session(&mut *session)
                        .await
                        .map_err(ApplyError::Driver)?;
                    if result.matched_count != 1 {
                        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
                    }
                    agents.push(current);
                }
                continue;
            }
        };
        let update = match changes.get(&current.id) {
            Some(change) => {
                let batch = materialize_logs(change, &request.observed_at)?;

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
