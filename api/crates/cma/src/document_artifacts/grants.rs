//! Recipient authority and the document revision share one durable transaction.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::{
    document_artifacts::{DocumentError, StoredDocument},
    SubjectId,
};
use mongodb::{
    bson::{doc, Document},
    options::{ReadConcern, WriteConcern},
    Collection,
};

pub(super) async fn exchange(
    rows: &Collection<Document>,
    expected: u64,
    record: &StoredDocument,
    encoded: Document,
    recipient: &SubjectId,
) -> Result<bool, DocumentError> {
    let expected = i64::try_from(expected).map_err(|_| DocumentError::Corrupt)?;
    let database = rows.client().database(&rows.namespace().db);
    let jitter = uuid::Uuid::new_v4().as_u128() as u64;
    for attempt in 0..5 {
        if attempt > 0 {
            let delay =
                (10_u64 << (attempt - 1).min(3)) + (jitter.rotate_left(attempt as u32) & 15);
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        let mut session = rows
            .client()
            .start_session()
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .await
            .map_err(|_| DocumentError::Unavailable)?;
        let result = async {
            crate::store::transaction_barrier::open(&mut session).await?;
            if crate::store::account_deletions::fence_subject_in_session(&database,recipient,&mut session).await? {
                return Ok::<_,mongodb::error::Error>(None);
            }
            let result = rows.replace_one(doc! {"_id": &record.metadata.id,"owner_subject":&record.metadata.owner_subject,"revision":expected},encoded.clone()).session(&mut session).await?;
            Ok(Some(result.matched_count == 1))
        }.await;
        let matched = match result {
            Ok(None) => {
                let _ = session.abort_guarded().await;
                return Err(DocumentError::ShareRecipientUnavailable);
            }
            Ok(Some(false)) => {
                let _ = session.abort_guarded().await;
                return Ok(false);
            }
            Ok(Some(true)) => true,
            Err(error) => {
                let _ = session.abort_guarded().await;
                if error.contains_label("TransientTransactionError") {
                    continue;
                }
                return Err(DocumentError::Unavailable);
            }
        };
        match session.commit_guarded().await {
            Ok(()) => return Ok(matched),
            // An unknown commit can already have persisted the grant receipt.
            // The exact operation key reconciles it; never repeat its effects.
            Err(error) if error.contains_label("UnknownTransactionCommitResult") => {
                return Err(DocumentError::Unavailable)
            }
            Err(error) if error.contains_label("TransientTransactionError") => continue,
            Err(_) => return Err(DocumentError::Unavailable),
        }
    }
    Err(DocumentError::Conflict)
}
