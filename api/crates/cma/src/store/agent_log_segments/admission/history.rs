//! One bounded source proof shared by reuse selection and publication.
use super::*;
use crate::store::{
    agent_log::scan_complete_log_with_session,
    agent_log_integrity::{
        integrity_document, prepare_append, AppendCapacity, LogSummary, SemanticCheckpoint,
    },
    agent_log_segments::{
        capture::{self, CapturedLog},
        publication::{self, PublicationIdentity},
        read_format,
        writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
    },
    agent_log_telemetry::ScanCaller,
    agents::agent_replacement_fields,
    sandbox_mongo::signed,
};
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind};
use mongodb::bson::Document;

pub(super) struct History {
    pub initial: AgentLogEntry,
    pub has_input: bool,
    source: Source,
}

enum Source {
    Legacy(Box<(LogSummary, SemanticCheckpoint)>),
    Segmented(Box<CapturedLog>),
}

impl History {
    pub async fn read(
        store: &MongoSandboxStore,
        agent: &Agent,
        row: &Document,
        scope: &mut TransactionScope<'_>,
    ) -> Result<Self, ApplyError> {
        if read_format(row)?.is_some() {
            let source = capture::capture_transaction(store, agent, scope).await?;
            let initial = capture::points_transaction(store, &source, &[1], scope)
                .await?
                .pop()
                .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
            return Ok(Self {
                initial,
                has_input: source.projection().has_owner_input,
                source: Source::Segmented(Box::new(source)),
            });
        }
        let mut entries = Vec::new();
        let summary = scan_complete_log_with_session(
            ScanCaller::CommandAdmission,
            store,
            &agent.id,
            agent.conversation.last_seq,
            scope.session(),
            |entry| {
                entries.push(entry);
                Ok(())
            },
        )
        .await?;
        let semantic = crate::store::agent_log_integrity::audit_checkpoint(
            store,
            agent,
            &entries,
            scope.session(),
        )
        .await?;
        Ok(Self {
            initial: entries
                .first()
                .cloned()
                .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?,
            has_input: entries
                .iter()
                .any(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. })),
            source: Source::Legacy(Box::new((summary, semantic))),
        })
    }

    pub async fn publish(
        &self,
        store: &MongoSandboxStore,
        admission: &AgentAdmission,
        current: &Agent,
        next: &Agent,
        logs: &[AgentLogEntry],
        scope: &mut TransactionScope<'_>,
    ) -> Result<Option<Recorded>, ApplyError> {
        let filter = doc! {
            "_id":current.id.as_str(), "channel_history_seal":{"$exists":false},
            "channel_reuse_cleanup_admission":{"$exists":false},
            "revision":signed(current.revision,"Agent revision").map_err(|e| ApplyError::Domain(SandboxStoreError::Corrupt(e)))?,
            "last_seq":signed(current.conversation.last_seq,"Agent log sequence").map_err(|e| ApplyError::Domain(SandboxStoreError::Corrupt(e)))?,
        };
        if let Source::Segmented(source) = &self.source {
            // Admission carries a durable creation key and event timestamp before
            // the transaction retry loop. Equal prompts with other keys remain distinct.
            let operation = PublicationIdentity {
                key: format!(
                    "reuse:{}",
                    crate::store::agent_admission_records::binding_id(admission)?
                ),
                fingerprint: publication::request_digest(&admission.fingerprint()?)?,
                appended_at: admission.event_at.clone(),
            };
            return Ok(Some(
                writer_commit::publish_captured(
                    store,
                    scope,
                    source,
                    PublicationInput {
                        operation: &operation,
                        current,
                        next,
                        logs,
                        capacity: AppendCapacity::Normal,
                        result_digest: publication::result_digest(&(
                            next,
                            logs,
                            admission.fingerprint()?,
                        ))?,
                    },
                    AggregateUpdate {
                        filter,
                        fields: agent_replacement_fields(next)?,
                    },
                )
                .await?,
            ));
        }
        let Source::Legacy(legacy) = &self.source else {
            unreachable!()
        };
        let (summary, semantic) = legacy.as_ref();
        let mut fields = agent_replacement_fields(next)?;
        if logs.is_empty() {
            // Metadata-only legacy reuse retains its existing integrity installation.
            fields = doc! {"agent_log_integrity_v1":integrity_document(next, summary, next.revision, semantic.clone())?};
        } else {
            let prepared = prepare_append(
                next,
                summary,
                logs,
                next.revision,
                semantic.after_append(logs, next)?,
                AppendCapacity::Normal,
            )?;
            fields.insert("agent_log_integrity_v1", prepared.integrity);
            fields.insert(
                "last_seq",
                signed(next.conversation.last_seq, "Agent log sequence")
                    .map_err(|e| ApplyError::Domain(SandboxStoreError::Corrupt(e)))?,
            );
            store
                .agent_log
                .clone_with_type::<mongodb::bson::RawDocumentBuf>()
                .insert_many(&prepared.documents)
                .session(scope.session())
                .await
                .map_err(classify_log_insert)?;
        }
        if scope
            .update_one(&store.agents, filter, doc! {"$set":fields})
            .await
            .map_err(ApplyError::Driver)?
            .matched_count
            != 1
        {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        Ok(None)
    }
}
