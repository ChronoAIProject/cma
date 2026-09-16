//! Generic deletion cannot consume a Channel root's retained recovery records.
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, Database,
};

/// Presence is enough to hold deletion, including malformed or null markers.
/// Their absence alone is insufficient: retained allocation evidence also wins.
pub(super) fn has_marker(document: &Document) -> bool {
    [
        "channel_creation",
        "channel_provisioning",
        "channel_home_provenance",
        "channel_home_prepare_evidence",
        "channel_retirement",
        "channel_storage_retention",
        "channel_history_seal",
        "channel_retirement_finalization",
    ]
    .iter()
    .any(|field| document.contains_key(field))
}

pub(super) fn root_allocation_filter() -> Document {
    doc! { "$or": [
        { "creates_sandbox": true },
        { "record.placement.kind": { "$nin": ["reuse_parent", "existing_agent"] } },
        { "sandbox_admission": { "$exists": true } },
    ] }
}

pub(super) async fn required(
    database: &Database,
    sandbox_id: &str,
    sandbox: Option<&Document>,
    session: Option<&mut ClientSession>,
) -> Result<bool, mongodb::error::Error> {
    if sandbox.is_some_and(has_marker) {
        return Ok(true);
    }
    let allocations: Collection<Document> =
        super::durability::majority_collection(database, "bridge_allocations");
    let mut target = root_allocation_filter();
    target.insert("record.placement.target.sandbox_id", sandbox_id);
    let query = allocations.find_one(doc! { "$or": [
        { "sandbox_admission.sandbox_id": sandbox_id },
        target,
    ] });
    let retained = match session {
        Some(session) => query.session(session).await?,
        None => query.await?,
    };
    Ok(retained.is_some())
}
