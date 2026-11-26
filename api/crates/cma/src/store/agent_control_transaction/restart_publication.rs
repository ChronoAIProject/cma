//! Atomically publish the restart and any terminal event from the original Stop.

use super::{
    signed, store_error, unavailable, unknown_control, ApplyError, DOCUMENT_SCHEMA_VERSION,
};
use crate::store::{
    agent_log::{log_id, read_entry},
    agent_log_integrity::{
        carry_integrity_for_revision, prepare_append, trusted_summary_at_head, TransactionLogError,
        INTEGRITY_FIELD,
    },
    agent_log_segments::{
        read_format,
        transaction_scope::TransactionScope,
        writer_commit::{self, AggregateUpdate, PublicationInput, Recorded},
    },
    sandbox_mongo::MongoSandboxStore,
    work_publication,
};
use layer2_core::sandbox::{
    store::{AgentControlStoreError, SandboxStoreError},
    AgentLogEntry,
};
use mongodb::{
    bson::{doc, Document, RawDocumentBuf},
    options::ReadConcern,
    ClientSession,
};

pub(super) enum RecordedRestart {
    Segmented(Box<Recorded>),
    Legacy(Box<AgentLogEntry>),
}

impl RecordedRestart {
    pub(super) async fn verify(
        &self,
        store: &MongoSandboxStore,
        session: &mut ClientSession,
    ) -> Result<(), AgentControlStoreError> {
        let matches = match self {
            Self::Segmented(publication) => publication
                .matches(store, session)
                .await
                .map_err(store_error)?,
            Self::Legacy(entry) => {
                let row = store
                    .agent_log
                    .find_one(doc! {"_id": log_id(&entry.agent_id, entry.seq)})
                    .read_concern(ReadConcern::snapshot())
                    .session(session)
                    .await
                    .map_err(unavailable)?;
                row.as_ref()
                    .map(read_entry)
                    .transpose()
                    .map_err(store_error)?
                    .as_ref()
                    == Some(entry.as_ref())
            }
        };
        if !matches {
            return Err(unknown_control("restart publication is absent"));
        }
        Ok(())
    }
}

pub(super) async fn record<'session>(
    store: &MongoSandboxStore,
    mut scope: TransactionScope<'session>,
    row: &Document,
    input: PublicationInput<'_>,
    mut update: AggregateUpdate,
) -> Result<(Option<RecordedRestart>, &'session mut ClientSession), ApplyError> {
    if !input.logs.is_empty() {
        scope
            .settle_owner_stop_budget(store, input.current, &input.operation.appended_at)
            .await
            .map_err(log_error)?;
    }
    if read_format(row).map_err(domain_error)?.is_some() {
        let recorded = writer_commit::publish_existing(store, &mut scope, input, update)
            .await
            .map_err(log_error)?;
        return Ok((
            Some(RecordedRestart::Segmented(Box::new(recorded))),
            scope.release(),
        ));
    }

    let session = scope.release();
    let interruption = if let Some(entry) = input.logs.first() {
        let summary = trusted_summary_at_head(store, input.current, row, session)
            .await
            .map_err(log_error)?
            .ok_or(ApplyError::Domain(
                AgentControlStoreError::RestartTurnUnsettled,
            ))?;
        let checkpoint = summary.semantic().ok_or(ApplyError::Domain(
            AgentControlStoreError::RestartTurnUnsettled,
        ))?;
        let semantic = checkpoint
            .after_append(input.logs, input.next)
            .map_err(domain_error)?;
        if !semantic.valid {
            return Err(ApplyError::Domain(
                AgentControlStoreError::RestartTurnUnsettled,
            ));
        }
        let prepared = prepare_append(
            input.current,
            &summary,
            input.logs,
            input.next.revision,
            semantic,
            input.capacity,
        )
        .map_err(domain_error)?;
        update.fields.insert(INTEGRITY_FIELD, prepared.integrity);
        update.fields.insert(
            "last_seq",
            signed(input.next.conversation.last_seq, "agent log sequence")
                .map_err(SandboxStoreError::Corrupt)
                .map_err(domain_error)?,
        );
        Some((entry, prepared.documents))
    } else {
        carry_integrity_for_revision(row, input.current, input.next, &mut update.fields)
            .map_err(domain_error)?;
        None
    };
    update
        .filter
        .insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
    let filter = work_publication::for_collection(&store.agents, update.filter)
        .map_err(mongodb::error::Error::custom)
        .map_err(ApplyError::Driver)?;
    let applied = store
        .agents
        .update_one(filter, doc! {"$set": update.fields})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if applied.matched_count != 1 {
        return Err(ApplyError::Domain(AgentControlStoreError::Conflict));
    }
    let Some((entry, documents)) = interruption else {
        return Ok((None, session));
    };
    for document in documents {
        store
            .agent_log
            .clone_with_type::<RawDocumentBuf>()
            .insert_one(document)
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
    }
    Ok((
        Some(RecordedRestart::Legacy(Box::new(entry.clone()))),
        session,
    ))
}

fn domain_error(error: SandboxStoreError) -> ApplyError {
    ApplyError::Domain(match error {
        SandboxStoreError::AgentLogCapacityExceeded => AgentControlStoreError::RestartTurnUnsettled,
        other => store_error(other),
    })
}

fn log_error(error: TransactionLogError) -> ApplyError {
    match error {
        TransactionLogError::Driver(error) => ApplyError::Driver(error),
        TransactionLogError::Domain(error) => domain_error(error),
    }
}
