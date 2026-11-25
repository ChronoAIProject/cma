use super::*;

pub(super) async fn started_at(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    admitted_at: &str,
) -> Result<Option<String>, SandboxStoreError> {
    let base = doc! {
        "sandbox_id": sandbox_id.as_str(),
        "record.appended_at": { "$gte": admitted_at },
        "record.kind.step.kind": "run_lifecycle_operation",
    };
    let mut progress_query = base.clone();
    progress_query.insert("record.kind.type", "work_step_completed");
    let latest_progress = store
        .sandbox_events
        .find_one(progress_query)
        .sort(doc! { "seq": -1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let after = latest_progress
        .as_ref()
        .map(read_event)
        .transpose()?
        .map(|event| event.seq)
        .unwrap_or(0);
    let after = signed(after, "sandbox event sequence").map_err(SandboxStoreError::Corrupt)?;
    let mut failure_query = base;
    failure_query.insert("seq", doc! { "$gt": after });
    failure_query.insert("record.kind.type", "work_step_failed");
    store
        .sandbox_events
        .find_one(failure_query)
        .sort(doc! { "seq": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_event)
        .transpose()
        .map(|event| event.map(|event| event.appended_at))
}
