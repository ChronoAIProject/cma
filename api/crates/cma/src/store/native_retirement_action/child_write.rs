use super::*;
use crate::store::sandbox_mongo::is_duplicate_key;
use crate::store::work_claim_authority::without_ledger_maintenance;

/// Low-level child creation follows the same parent fence as public admissions.
pub(in crate::store) async fn insert_child(
    store: &MongoSandboxStore,
    collection: &mongodb::Collection<Document>,
    parent: &SandboxId,
    row: Document,
) -> Result<(), SandboxStoreError> {
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        let inserted = async {
            start_transaction(&mut session).await?;
            if !fence_obligation(store, parent, &mut session).await? {
                return Ok(false);
            }
            collection
                .insert_one(row.clone())
                .session(&mut session)
                .await?;
            Ok(true)
        }
        .await;
        match inserted {
            Ok(true) => {}
            Ok(false) => {
                let _ = session.abort_guarded().await;
                return Err(SandboxStoreError::Conflict);
            }
            Err(error) => {
                let _ = session.abort_guarded().await;
                if retryable_body_error(&error) {
                    continue;
                }
                if is_duplicate_key(&error) {
                    return Err(SandboxStoreError::Duplicate);
                }
                return Err(unavailable(error));
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => {}
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(unavailable(error))
            }
        }
    }
    Err(SandboxStoreError::Conflict)
}

pub(in crate::store) fn adds_obligation(previous: &Agent, next: &Agent) -> bool {
    next.work
        .cleanup_debt
        .iter()
        .any(|debt| !previous.work.cleanup_debt.contains(debt))
}

/// Serialize publication of a new obligation with deletion and maintenance admission.
/// Both writers update the same parent in their own transactions. A snapshot
/// read alone would allow a writer that started before custody to publish.
pub(in crate::store) async fn fence_obligation(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<bool, mongodb::error::Error> {
    let result = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(without_ledger_maintenance(doc! {
                "_id": sandbox.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                FIELD: { "$exists": false },
            })),
            doc! { "$inc": { CHILD_WRITE_FENCE: 1_i64 } },
        )
        .session(&mut *session)
        .await?;
    if result.matched_count == 1 {
        return Ok(true);
    }
    // The low-level Agent store also supports rows whose parent was already
    // absent. Only absence in this transaction permits that legacy operation.
    let parent = store
        .sandboxes
        .find_one(doc! { "_id": sandbox.as_str() })
        .projection(doc! { "_id": 1 })
        .session(session)
        .await?;
    Ok(parent.is_none())
}
