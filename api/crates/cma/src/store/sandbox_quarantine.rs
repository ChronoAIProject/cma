//! Hold undecodable retained Sandboxes in place for operator repair.

use layer2_core::sandbox::Sandbox;
use mongodb::{
    bson::{doc, Document},
    Collection,
};
use sha2::{Digest, Sha256};

pub(super) const FIELD: &str = "retained_sandbox_quarantine";

pub(super) fn contains(row: &Document) -> bool {
    row.contains_key(FIELD)
}

pub(super) fn exclude(mut filter: Document) -> Document {
    filter.insert(FIELD, doc! {"$exists": false});
    filter
}

/// A second starter or a repair may change the row after the cursor reads it.
/// Reread a missed compare-and-set before deciding how to route that Sandbox.
pub(super) async fn retained(
    rows: &Collection<Document>,
    mut row: Document,
) -> Result<Option<Sandbox>, String> {
    for _ in 0..super::sandbox_mongo::TRANSACTION_ATTEMPTS {
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
        let mut predicate = exclude(identity.clone());
        predicate.insert("$expr", doc! {"$eq": ["$$ROOT", {"$literal": &row}]});
        let marker =
            doc! {"reason": "invalid_retained_sandbox", "detected_at": layer2_core::now_rfc3339()};
        let marked = rows
            .update_one(predicate, doc! {"$set": { (FIELD): marker }})
            .await
            .map_err(|_| "cannot quarantine retained Sandbox")?;
        if marked.modified_count == 1 {
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
        let Some(current) = rows
            .find_one(identity)
            .await
            .map_err(|_| "cannot recheck retained Sandbox after quarantine conflict")?
        else {
            return Ok(None);
        };
        row = current;
    }
    Err("retained Sandbox changed during quarantine".into())
}
