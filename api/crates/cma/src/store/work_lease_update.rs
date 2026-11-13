//! Preserve revision-stable renewal when an older content snapshot is saved.

use mongodb::bson::{doc, Bson, Document};

/// Read-only reconciliation accepts the exact committed content after renewal
/// or expiry. It never authorizes another mutation or another effect dispatch.
pub(super) fn sandbox_matches(
    actual: Option<&layer2_core::sandbox::Sandbox>,
    expected: &layer2_core::sandbox::Sandbox,
) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    if actual
        .work
        .lease
        .iter()
        .chain(expected.work.lease.iter())
        .any(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none())
    {
        return false;
    }
    let mut expected = expected.clone();
    if let (Some(old), Some(now)) = (&mut expected.work.lease, &actual.work.lease) {
        if old.holder == now.holder && old.fence == now.fence && old.expires_at <= now.expires_at {
            old.expires_at.clone_from(&now.expires_at);
        }
    }
    actual == &expected
}

pub(super) fn workspace_matches(
    actual: Option<&layer2_core::sandbox::Workspace>,
    expected: &layer2_core::sandbox::Workspace,
) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    if actual
        .work
        .lease
        .iter()
        .chain(expected.work.lease.iter())
        .any(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at).is_none())
    {
        return false;
    }
    let mut expected = expected.clone();
    if let (Some(old), Some(now)) = (&mut expected.work.lease, &actual.work.lease) {
        if old.holder == now.holder && old.fence == now.fence && old.expires_at <= now.expires_at {
            old.expires_at.clone_from(&now.expires_at);
        }
    }
    actual == &expected
}

/// Keep the driver's existing increment validation and overflow behavior.
/// Both writes belong to the caller's transaction; no intermediate state is
/// published, and an increment does not change the content-revision filter.
pub(super) async fn with_fence(
    rows: &mongodb::Collection<Document>,
    filter: Document,
    fields: Option<Document>,
    counter: &str,
    session: &mut mongodb::ClientSession,
) -> Result<mongodb::results::UpdateResult, mongodb::error::Error> {
    let result = rows
        .update_one(filter.clone(), doc! {"$inc":{counter:1_i64}})
        .session(&mut *session)
        .await?;
    match fields {
        Some(fields) if result.matched_count == 1 => {
            rows.update_one(filter, replacement(fields))
                .session(session)
                .await
        }
        _ => Ok(result),
    }
}

/// Evaluate preservation at the write, including for nontransactional callers.
/// A different holder/fence or a cleared lease still uses the submitted value;
/// the caller's acquisition/publication filter remains responsible for authority.
pub(super) fn replacement(fields: Document) -> Vec<Document> {
    let mut update = fields
        .iter()
        .map(|(key, value)| (key.clone(), Bson::Document(doc! {"$literal":value.clone()})))
        .collect::<Document>();
    if let Some(lease) = fields
        .get_document("record")
        .ok()
        .and_then(|record| record.get_document("work").ok())
        .and_then(|work| work.get_document("lease").ok())
    {
        let same = doc! {"$and":[
            {"$eq":["$record.work.lease.holder",{"$literal":lease.get("holder").cloned().unwrap_or(Bson::Null)}]},
            {"$eq":["$record.work.lease.fence",{"$literal":lease.get("fence").cloned().unwrap_or(Bson::Null)}]},
        ]};
        let record = fields.get_document("record").expect("lease has a record");
        let preserved = doc! {"$mergeObjects":["$$nextRecord",{"work":{
            "$mergeObjects":["$$nextRecord.work",{"lease":{
                "$mergeObjects":["$$nextRecord.work.lease",
                    {"expires_at":"$record.work.lease.expires_at"}]
            }}]
        }}]};
        // Bind the payload once so the update does not double the BSON record
        // size merely to select between retaining and completing a claim.
        update.insert(
            "record",
            doc! {"$let":{"vars":{"nextRecord":{"$literal":record.clone()}},
            "in":{"$cond":[same.clone(),preserved,"$$nextRecord"]}}},
        );
        update.insert("work_lease_expires_at",doc! {"$cond":[same,
            "$work_lease_expires_at",{"$literal":fields.get("work_lease_expires_at").cloned().unwrap_or(Bson::Null)}]});
    }
    vec![doc! {"$set":update}]
}
