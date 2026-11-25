//! Bind an admitted retry to the new work fence in the claim transaction.
use super::*;
use layer2_core::sandbox::Sandbox;

pub(in crate::store) enum ClaimPark {
    Proceed,
    Hold,
    Retry(Document),
}

pub(in crate::store) async fn for_claim(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    parent: &Sandbox,
    claimed: &Agent,
    session: &mut ClientSession,
) -> Result<ClaimPark, SandboxStoreError> {
    if !store.agent_inspection_parking {
        return Ok(ClaimPark::Proceed);
    }
    let Ok(marker) = row.get_document(FIELD) else {
        return Ok(ClaimPark::Proceed);
    };
    if marker.get_i64("version").ok() != Some(1)
        || marker.get_bool("inspection_confirmed").unwrap_or(false)
        || marker.get_str("id").is_err()
        || marker.get_str("step").ok() != Some(current.work.step.operation_name())
        || marker.get_str("ready_at").ok() != Some(current.work.ready_at.as_str())
        || marker.get_i64("attempts").ok() != Some(i64::from(current.work.attempts))
        || marker
            .get_i64("fence")
            .ok()
            .and_then(|fence| u64::try_from(fence).ok())
            != Some(current.work.next_fence)
        || marker
            .get_document("binding")
            .ok()
            .and_then(|binding| {
                mongodb::bson::deserialize_from_document::<RuntimeBinding>(binding.clone()).ok()
            })
            .as_ref()
            != parent.runtime.as_ref()
    {
        return Ok(ClaimPark::Proceed);
    }
    if !marker.get_bool("retry").unwrap_or(false) {
        let Ok(probe_at) = marker.get_datetime("probe_at") else {
            return Ok(ClaimPark::Hold);
        };
        let Some((_, now)) =
            super::super::agent_failure_pacing::ownership::server_row(store, &current.id, session)
                .await?
        else {
            return Ok(ClaimPark::Hold);
        };
        if *probe_at > now {
            return Ok(ClaimPark::Hold);
        }
    }
    let mut consumed = marker.clone();
    consumed.remove("probe_at");
    consumed.remove("retry");
    consumed.insert("probe_spent", true);
    consumed.insert("retry_claimed", true);
    consumed.insert("id", uuid::Uuid::new_v4().to_string());
    consumed.insert("ready_at", claimed.work.ready_at.clone());
    consumed.insert(
        "fence",
        signed(claimed.work.next_fence, "retry work fence").map_err(SandboxStoreError::Corrupt)?,
    );
    Ok(ClaimPark::Retry(consumed))
}
