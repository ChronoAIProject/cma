//! Retain-only seal of material history. Permanent references may be extended
//! by management receipts and late-upload tombstones, never removed or expired.
use super::*;
use crate::sandbox::channel_retirement::model::ChannelRetirementSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistorySeal {
    pub schema_version: u16,
    pub policy: ChannelHistoryPolicy,
    pub sandbox_id: String,
    pub sandbox_created_at: String,
    pub tree_fingerprint: String,
    pub storage_retention_sha256: String,
    pub reference_count_at_seal: u64,
    pub references_sha256_at_seal: String,
    pub reference_ids_at_seal: Vec<String>,
    pub material: inventory::MaterialInventory,
    pub sealed_at: String,
}
impl HistorySeal {
    pub(crate) fn digest(&self) -> Result<String, AllocationError> {
        digest(&("channel-retained-history-seal-v1", self))
    }
    pub(crate) fn require(
        &self,
        snapshot: &ChannelRetirementSnapshot,
    ) -> Result<(), AllocationError> {
        if self.schema_version != 1
            || Some(&self.policy) != snapshot.history.policy.as_ref()
            || self.policy != ChannelHistoryPolicy::capture(&snapshot.allocation)?
            || self.sandbox_id != snapshot.sandbox.id.as_str()
            || self.sandbox_created_at != snapshot.sandbox.created_at
            || self.tree_fingerprint != snapshot.tree.fingerprint
            || self.storage_retention_sha256
                != snapshot
                    .retention
                    .as_ref()
                    .ok_or(AllocationError::Corrupt)?
                    .digest()?
            || self.reference_count_at_seal == 0
            || self.reference_count_at_seal != self.reference_ids_at_seal.len() as u64
            || snapshot.history.reference_count < self.reference_count_at_seal
            || self.references_sha256_at_seal.len() != 64
            || layer2_core::seconds_from_rfc3339(&self.sealed_at).is_none()
            || self.sealed_at < snapshot.sandbox.created_at
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
}

pub(crate) async fn seal(
    database: &Database,
    snapshot: &ChannelRetirementSnapshot,
    at: &str,
    session: &mut ClientSession,
) -> Result<Result<HistorySeal, HistoryHold>, MutationError> {
    let Some(policy) = &snapshot.history.policy else {
        return Ok(Err(HistoryHold::LegacyPolicyMissing));
    };
    if let Some(hold) = snapshot.history.hold {
        return Ok(Err(hold));
    }
    let material = match inventory::capture(database, snapshot, session).await? {
        Ok(inventory) => inventory,
        Err(hold) => return Ok(Err(hold)),
    };
    let seal = HistorySeal {
        schema_version: 1,
        policy: policy.clone(),
        sandbox_id: snapshot.sandbox.id.as_str().into(),
        sandbox_created_at: snapshot.sandbox.created_at.clone(),
        tree_fingerprint: snapshot.tree.fingerprint.clone(),
        storage_retention_sha256: snapshot
            .retention
            .as_ref()
            .ok_or(AllocationError::Corrupt)?
            .digest()?,
        reference_count_at_seal: snapshot.history.reference_count,
        references_sha256_at_seal: snapshot
            .history
            .reference_sha256
            .clone()
            .ok_or(AllocationError::Corrupt)?,
        reference_ids_at_seal: inventory::rows(
            database,
            REFERENCES,
            doc! {"allocation_id": &snapshot.allocation.id},
            session,
        )
        .await?
        .iter()
        .map(|row| read_reference(row)?.id())
        .collect::<Result<Vec<_>, AllocationError>>()?,
        material,
        sealed_at: at.into(),
    };
    seal.require(snapshot)?;
    let marker = seal.digest()?;
    let rows: Collection<Document> = majority_collection(database, "bridge_allocations");
    if rows.update_one(doc! {"_id": &snapshot.allocation.id, "channel_history_seal": {"$exists": false}},
        doc! {"$set": {"channel_history_seal": record_bson(&seal).map_err(|_| AllocationError::Corrupt)?}})
        .session(&mut *session).await?.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    let agents: Collection<Document> = majority_collection(database, "agents");
    if agents
        .update_one(
            doc! {"_id": snapshot.allocation.placement.target().agent_id.as_str(),
            "channel_history_seal": {"$exists": false}},
            doc! {"$set": {"channel_history_seal": &marker}},
        )
        .session(&mut *session)
        .await?
        .matched_count
        != 1
    {
        return Err(AllocationError::Conflict.into());
    }
    for row in &seal.material.commands {
        freeze(database, "agent_commands", &row.id, &marker, session).await?;
    }
    for row in &seal.material.outbox {
        freeze(database, "bridge_outbox_items", &row.id, &marker, session).await?;
    }
    Ok(Ok(seal))
}

async fn freeze(
    database: &Database,
    collection: &str,
    id: &str,
    digest: &str,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    if majority_collection::<Document>(database, collection)
        .update_one(
            doc! {"_id": id, "channel_history_seal": {"$exists": false}},
            doc! {"$set": {"channel_history_seal": digest}},
        )
        .session(session)
        .await?
        .matched_count
        != 1
    {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
