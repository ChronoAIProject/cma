//! Bounded original-request candidates for one disconnected Agent after restart.
//! An empty result is not KnownAbort and cannot authorize ordinary reconnect.
use super::*;
use layer2_core::AgentId;

pub(in crate::store::agent_capacity_recovery) async fn page(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
) -> Result<Vec<RecoveryRequest>, RecoveryError> {
    if agent_id.as_str().is_empty() || agent_id.as_str().len() > 512 {
        return Err(RecoveryError::InvalidEvidence);
    }
    let mut rows = records::collection(store)
        .find(doc! {"agent_id": agent_id.as_str(), "format_version": 2_i64})
        .sort(doc! {"_id": 1})
        .hint(Hint::Name("agent_capacity_recovery_agent".into()))
        .collation(Collation::builder().locale("simple").build())
        .projection(doc! {"_id": 1, "format_version": 1, "agent_id": 1,
        "work.request": 1, "work.revision": 1, "work.phase": 1})
        .limit(i64::from(PAGE_SIZE + 1))
        .batch_size(PAGE_SIZE + 1)
        .max_time(Duration::from_millis(1000))
        .await
        .map_err(|_| RecoveryError::Unavailable)?;
    let mut requests = Vec::new();
    let mut bytes = 0_usize;
    let mut previous: Option<String> = None;
    while let Some(row) = rows
        .try_next()
        .await
        .map_err(|_| RecoveryError::Unavailable)?
    {
        let size = bson::serialize_to_vec(&row)
            .map_err(|_| RecoveryError::InvalidEvidence)?
            .len();
        bytes = bytes
            .checked_add(size)
            .filter(|n| *n <= 128 * 1024)
            .ok_or(RecoveryError::Unavailable)?;
        if requests.len() == PAGE_SIZE as usize {
            return Err(RecoveryError::Unavailable);
        }
        let work = row
            .get_document("work")
            .map_err(|_| RecoveryError::InvalidEvidence)?;
        let request: RecoveryRequest = bson::deserialize_from_document(
            work.get_document("request")
                .map_err(|_| RecoveryError::InvalidEvidence)?
                .clone(),
        )
        .map_err(|_| RecoveryError::InvalidEvidence)?;
        let key = request.key()?;
        if &request.agent_id != agent_id
            || row.get_i64("format_version").ok() != Some(2)
            || row.get_str("agent_id").ok() != Some(agent_id.as_str())
            || row.get_str("_id").ok() != Some(key.as_str())
            || !work.get_i64("revision").is_ok_and(|revision| revision >= 1)
            || !matches!(
                work.get_str("phase").ok(),
                Some("admitted" | "attaching" | "completed" | "superseded")
            )
            || previous.as_ref().is_some_and(|previous| key <= *previous)
        {
            return Err(RecoveryError::InvalidEvidence);
        }
        previous = Some(key);
        requests.push(request);
    }
    Ok(requests)
}
