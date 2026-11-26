//! Detached acceptance receipts have no owner lookup or Sandbox lineage.
use super::{
    account_erasure::Error,
    directory::USERS,
    durability::majority_collection,
    legal_consents,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    schema::ensure_collections,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, Document},
    ClientSession, Database,
};
use ring::rand::{SecureRandom, SystemRandom};

pub(super) const COLLECTION: &str = "retained_legal_consents";
pub(super) const ACTIONS: &[&str] = &["createCollection", "insert"];

fn unavailable() -> Error {
    Error::Store("retaining legal consent during account erasure failed".into())
}

pub(super) async fn prepare(db: &Database) -> Result<(), Error> {
    ensure_collections(db, &[COLLECTION])
        .await
        .map_err(|_| unavailable())
}

pub(super) async fn erase_user(db: &Database, subject: &str) -> Result<bool, Error> {
    let mut session = db
        .client()
        .start_session()
        .await
        .map_err(|_| unavailable())?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable())?;
    let removed = match remove_and_retain(db, subject, &mut session).await {
        Ok(true) => true,
        result => {
            session.abort_guarded().await.map_err(|_| unavailable())?;
            return result;
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(removed),
        // Another attempt must observe the source row again. A successful
        // commit has already deleted it, so there is no receipt to duplicate.
        CommitOutcome::Retry(_) | CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
            Err(unavailable())
        }
    }
}

async fn remove_and_retain(
    db: &Database,
    subject: &str,
    session: &mut ClientSession,
) -> Result<bool, Error> {
    let Some(row) = majority_collection::<Document>(db, USERS)
        .find_one_and_delete(doc! { "_id": subject })
        .projection(doc! { "_id": 0, "document_schema_version": 1, legal_consents::FIELD: 1 })
        .session(&mut *session)
        .await
        .map_err(|_| unavailable())?
    else {
        return Ok(false);
    };
    let acceptances = legal_consents::decode(row).map_err(|_| Error::InvalidRecord(USERS))?;
    if acceptances.is_empty() {
        return Ok(true);
    }
    // A fresh random identifier has no inputs derived from the directory row.
    // Store it in the unique composite key to avoid an extra receipt field.
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| unavailable())?;
    let account_id = Binary {
        subtype: BinarySubtype::Generic,
        bytes: bytes.to_vec(),
    };
    let receipts = acceptances.into_iter().map(|acceptance| {
        doc! {
            "_id": { "anonymous_account_id": account_id.clone(), "terms_version": acceptance.version },
            "accepted_at": acceptance.accepted_at,
        }
    });
    majority_collection::<Document>(db, COLLECTION)
        .insert_many(receipts)
        .session(session)
        .await
        .map_err(|_| unavailable())?;
    Ok(true)
}
