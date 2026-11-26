//! Apply one validated publication plan in the caller's transaction. Any error
//! requires aborting the whole transaction, including the caller's domain writes.
use super::*;
use crate::store::agent_log_segments::{
    collections::collection, corrupt, EVIDENCE, FORMAT, HEADS, PROJECTIONS, PUBLICATIONS, SEGMENTS,
};
use crate::store::{
    agent_log_integrity::{INTEGRITY_FIELD, QUARANTINE_FIELD},
    agents::agent_replacement_fields,
    sandbox_mongo::is_duplicate_key,
};
use mongodb::bson::{self, doc, Bson};

pub(super) async fn persist(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError> {
    scope.require(&prepared.transaction)?;
    if store.agents.namespace().db != prepared.database {
        return Err(SandboxStoreError::Conflict.into());
    }
    let (filter, update) = aggregate_update(&prepared, aggregate)?;
    let session = scope.session();
    let database = store.client.database(&store.agents.namespace().db);
    if let Some(permit) = &prepared.permit {
        permit
            .consume(
                &database,
                prepared.admission.as_ref().ok_or_else(corrupt)?,
                session,
            )
            .await?;
    }
    let written = store
        .agents
        .update_one(filter, update)
        .session(&mut *session)
        .await
        .map_err(classify)?;
    if written.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }

    // Exact original buffers used for planning are inserted unchanged. There is
    // one global sequence interval and no independently durable successor/seal.
    if !prepared.batch.rows().is_empty() {
        store
            .agent_log
            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
            .insert_many(prepared.batch.rows())
            .session(&mut *session)
            .await
            .map_err(classify)?;
    }
    if let Some(seal) = &prepared.plan.seal {
        collection(store, SEGMENTS)
            .insert_one(bson::serialize_to_document(seal).map_err(|_| corrupt())?)
            .session(&mut *session)
            .await
            .map_err(classify)?;
    }
    let head = bson::serialize_to_document(&prepared.plan.head).map_err(|_| corrupt())?;
    match prepared.source {
        SourceFormat::Legacy => {
            collection(store, HEADS)
                .insert_one(head)
                .session(&mut *session)
                .await
                .map_err(classify)?;
            collection(store, PROJECTIONS)
                .insert_one(prepared.delta.projection)
                .session(&mut *session)
                .await
                .map_err(classify)?;
        }
        SourceFormat::Segmented => {
            let heads = collection(store, HEADS);
            let matched = heads
                .replace_one(
                    bson::serialize_to_document(&prepared.before).map_err(|_| corrupt())?,
                    head,
                )
                .session(&mut *session)
                .await
                .map_err(classify)?
                .matched_count;
            if matched != 1 {
                return Err(SandboxStoreError::Conflict.into());
            }
            let matched = collection(store, PROJECTIONS).replace_one(doc! {
                "_id":&prepared.before.agent_id,"format_version":prepared.before.projection_format,
                "incarnation":&prepared.before.incarnation,"evidence_generation":&prepared.before.evidence_generation,
                "through_seq":prepared.delta.expected_through,"head_revision":prepared.delta.expected_revision,
            }, prepared.delta.projection).session(&mut *session).await.map_err(classify)?.matched_count;
            if matched != 1 {
                return Err(SandboxStoreError::Conflict.into());
            }
        }
    }
    if let Some(proof) = prepared.active_proof {
        proof.persist(store, session).await?;
    }
    for write in prepared.evidence {
        let evidence = collection(store, EVIDENCE);
        if let Some(before) = write.before {
            if evidence
                .replace_one(before, write.after)
                .session(&mut *session)
                .await
                .map_err(classify)?
                .matched_count
                != 1
            {
                return Err(SandboxStoreError::Conflict.into());
            }
        } else {
            evidence
                .insert_one(write.after)
                .session(&mut *session)
                .await
                .map_err(classify)?;
        }
    }
    collection(store, PUBLICATIONS)
        .insert_one(bson::serialize_to_document(&prepared.receipt).map_err(|_| corrupt())?)
        .session(session)
        .await
        .map_err(classify)?;
    Ok(prepared.receipt)
}

fn aggregate_update(
    prepared: &PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<(Document, Document), SandboxStoreError> {
    let current = &prepared.current;
    let next = &prepared.next;
    let mut fields = aggregate.fields;
    // The caller provides its full existing Agent replacement fields. Require
    // exact agreement with the planned result, including derived query fields.
    // Extra caller-owned checkpoint/domain fields remain in the same update.
    let canonical = agent_replacement_fields(next)?;
    for (field, expected) in &canonical {
        if fields.get(field) != Some(expected) {
            return Err(corrupt());
        }
    }
    for key in fields.keys() {
        let base = key.split('.').next().unwrap_or(key);
        if key == "_id"
            || key.starts_with('$')
            || (key.contains('.') && canonical.contains_key(base))
            || base.starts_with("agent_log_")
            || key == "document_schema_version"
        {
            return Err(corrupt());
        }
    }
    let head = &prepared.plan.head;
    let next_seq = i64::try_from(next.conversation.last_seq).map_err(|_| corrupt())?;
    if fields
        .get("last_seq")
        .is_some_and(|value| value != &Bson::Int64(next_seq))
    {
        return Err(corrupt());
    }
    fields.insert("last_seq", next_seq);
    fields.insert("document_schema_version", 2_i64);
    fields.insert("agent_log_format", FORMAT);
    fields.insert("agent_log_incarnation", &head.incarnation);
    fields.insert("agent_log_head_revision", head.revision);
    let source_format = match prepared.source {
        SourceFormat::Legacy => doc! {
            "document_schema_version":1_i64,"agent_log_format":{"$exists":false},
            "agent_log_incarnation":{"$exists":false},"agent_log_head_revision":{"$exists":false},
        },
        SourceFormat::Segmented => doc! {
            "document_schema_version":2_i64,"agent_log_format":FORMAT,
            "agent_log_incarnation":&prepared.before.incarnation,"agent_log_head_revision":prepared.before.revision,
        },
    };
    let bound = doc! {
        "_id":current.id.as_str(),"revision":i64::try_from(current.revision).map_err(|_| corrupt())?,
        "record.revision":i64::try_from(current.revision).map_err(|_| corrupt())?,
        "last_seq":prepared.before.last_seq,"record.conversation.last_seq":prepared.before.last_seq,
        "creation_key":&current.creation_key,"creation_fingerprint":&current.creation_fingerprint,
        "record.created_at":&current.created_at,"owner":current.owner.as_str(),
        "sandbox_id":current.sandbox_id.as_str(),"workspace_id":current.workspace_id.as_str(),
        "channel_history_seal":{"$exists":false},"channel_reuse_cleanup_execution":{"$exists":false},
        QUARANTINE_FIELD:{"$exists":false},
    };
    Ok((
        doc! {"$and":[aggregate.filter, bound, source_format]},
        doc! {"$set":fields,"$unset":{INTEGRITY_FIELD:""}},
    ))
}

fn classify(error: mongodb::error::Error) -> TransactionLogError {
    if !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
        && is_duplicate_key(&error)
    {
        SandboxStoreError::Conflict.into()
    } else {
        TransactionLogError::Driver(error)
    }
}

#[cfg(test)]
#[path = "writer_persist_tests.rs"]
mod tests;
