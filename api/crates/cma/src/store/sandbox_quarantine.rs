//! Hold undecodable retained Sandboxes in place for operator repair.

use layer2_core::sandbox::Sandbox;
use mongodb::{
    bson::{doc, Document},
    Collection,
};
use sha2::{Digest, Sha256};

use super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, TRANSACTION_ATTEMPTS,
};
use super::transaction_terminal::GuardedTerminal as _;

pub(super) const FIELD: &str = "retained_sandbox_quarantine";

pub(super) fn contains(row: &Document) -> bool {
    row.contains_key(FIELD)
}

pub(super) fn exclude(mut filter: Document) -> Document {
    filter.insert(FIELD, doc! {"$exists": false});
    filter
}

/// The cursor establishes the candidate. Decode again in the write transaction
/// so a concurrent repair cannot be mistaken for the corrupt snapshot.
pub(super) async fn retained(
    rows: &Collection<Document>,
    row: Document,
) -> Result<Option<Sandbox>, String> {
    if contains(&row) {
        return Ok(None);
    }
    if let Ok(sandbox) = super::sandboxes::read_sandbox(&row) {
        return Ok(Some(sandbox));
    }
    let id = row
        .get("_id")
        .ok_or("retained Sandbox is missing its Mongo identity")?;
    let identity = doc! {"_id": {"$eq": id}};
    let marker =
        doc! {"reason": "invalid_retained_sandbox", "detected_at": layer2_core::now_rfc3339()};
    let update = doc! {"$set": { (FIELD): marker }};
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = rows
            .client()
            .start_session()
            .await
            .map_err(|_| "cannot start retained Sandbox quarantine session")?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err("cannot start retained Sandbox quarantine transaction".into());
        }
        let current = match rows
            .find_one(exclude(identity.clone()))
            .session(&mut session)
            .await
        {
            Ok(current) => current,
            Err(error) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                continue;
            }
            Err(_) => {
                let _ = session.abort_guarded().await;
                return Err("cannot recheck retained Sandbox before quarantine".into());
            }
        };
        match current.as_ref().map(super::sandboxes::read_sandbox) {
            None => {
                let _ = session.abort_guarded().await;
                return Ok(None);
            }
            Some(Ok(sandbox)) => {
                let _ = session.abort_guarded().await;
                return Ok(Some(sandbox));
            }
            Some(Err(_)) => {}
        }
        if let Err(error) = rows
            .update_one(exclude(identity.clone()), update.clone())
            .session(&mut session)
            .await
        {
            let _ = session.abort_guarded().await;
            if is_transient(&error) {
                continue;
            }
            return Err("cannot quarantine retained Sandbox".into());
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => {
                // Never put the original BSON, owner, or decoder output into telemetry.
                let identity_hash = format!("{:x}", Sha256::digest(id.to_string().as_bytes()));
                tracing::error!(
                    alert = "CmaRetainedSandboxQuarantined",
                    sandbox_identity_sha256 = %identity_hash,
                    "retained Sandbox quarantined during startup"
                );
                metrics::counter!("cma_retained_sandbox_quarantines_total").increment(1);
                return Ok(None);
            }
            CommitOutcome::Retry(_) | CommitOutcome::Unknown(_) => continue,
            CommitOutcome::Failed(_) => {
                return Err("cannot commit retained Sandbox quarantine".into())
            }
        }
    }
    Err("retained Sandbox changed during quarantine".into())
}
