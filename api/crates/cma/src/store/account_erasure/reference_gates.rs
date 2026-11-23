//! Preserve rollout and lease authority while removing personal cohort references.
use super::{array_mentions, outcome, Error, Redaction};
use crate::store::{
    agent_log_segments::writer_gate::{GateName, GateRecord},
    durability::majority_collection,
};
use mongodb::{
    bson::{self, doc, Bson, DateTime, Document},
    Database,
};
use std::collections::BTreeSet;

const COLLECTION: &str = "agent_log_gates";
fn slot_matches(holder: &str, agents: &BTreeSet<String>) -> bool {
    agents
        .iter()
        .any(|agent| holder.starts_with(&format!("{}:{agent}:", agent.len())))
}

pub(super) async fn redact(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
    agents: &BTreeSet<String>,
    now: &str,
) -> Result<Redaction, Error> {
    let policy = row.get_document("policy").ok();
    let matches = array_mentions(policy.and_then(|p| p.get_array("owners").ok()), subjects)
        || array_mentions(policy.and_then(|p| p.get_array("agents").ok()), agents)
        || row.get_array("slots").ok().is_some_and(|slots| {
            slots.iter().any(|value| {
                value
                    .as_document()
                    .and_then(|s| s.get_str("holder").ok())
                    .is_some_and(|holder| slot_matches(holder, agents))
            })
        });
    if !matches {
        return Ok(Redaction::Unchanged);
    }
    let invalid = || Error::InvalidRecord(COLLECTION);
    let name = match row.get_str("_id").map_err(|_| invalid())? {
        "agent_log_segments_v1" => GateName::Segments,
        "agent_log_capacity_recovery_v1" => GateName::CapacityRecovery,
        _ => return Err(invalid()),
    };
    let gate = GateRecord::read(name, row.clone()).map_err(|_| invalid())?;
    let now = DateTime::parse_rfc3339_str(now).map_err(|_| invalid())?;
    let expired: Vec<_> = gate
        .slots
        .iter()
        .filter(|s| slot_matches(&s.holder, agents))
        .map(|s| s.expires_at)
        .collect();
    // A slow caller clock can only delay cleanup. A fast clock is checked again
    // against server time in the final compare and exchange below.
    if expired.iter().any(|expires_at| *expires_at > now) {
        return Ok(Redaction::Waiting);
    }
    let mut next = gate.clone();
    next.policy.owners.retain(|owner| !subjects.contains(owner));
    next.policy.agents.retain(|agent| !agents.contains(agent));
    if next.policy.owners.is_empty()
        && next.policy.agents.is_empty()
        && next.policy.basis_points == 0
    {
        next.policy.enabled = false;
    }
    next.slots
        .retain(|slot| !slot_matches(&slot.holder, agents));
    next.revision = gate.revision.checked_add(1).ok_or_else(invalid)?;
    let replacement = bson::serialize_to_document(&next).map_err(|_| invalid())?;
    GateRecord::read(name, replacement.clone()).map_err(|_| invalid())?;
    // Claim changes the admission fence; renew and consume change only slots.
    // Comparing both avoids overwriting a live permit at the same revision.
    let mut filter = doc! {"_id":name.as_str(),"format_version":1_i64,"revision":gate.revision,
    "admission_fence":gate.admission_fence,"slots":{"$eq":row.get("slots").cloned().ok_or_else(invalid)?}};
    if !expired.is_empty() {
        let conditions: Vec<Bson> = expired
            .into_iter()
            .map(|expires_at| doc! {"$lte":[expires_at,"$$NOW"]}.into())
            .collect();
        filter.insert("$expr", doc! {"$and":conditions});
    }
    let result = majority_collection::<Document>(db, COLLECTION)
        .replace_one(filter, replacement)
        .await?;
    Ok(outcome(result.matched_count == 1))
}
