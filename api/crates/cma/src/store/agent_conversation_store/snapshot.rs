use super::*;

pub(super) async fn snapshot_session(
    store: &MongoSandboxStore,
    snapshot_time: Option<mongodb::bson::Timestamp>,
) -> Result<mongodb::ClientSession, SandboxStoreError> {
    let session = store.client.start_session().snapshot(true);
    let session = match snapshot_time {
        Some(snapshot_time) => session.snapshot_time(snapshot_time),
        None => session,
    };
    let session = session
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    Ok(session)
}

pub(super) fn snapshot_error(error: ApplyError) -> SandboxStoreError {
    match error {
        ApplyError::Domain(error) => error,
        ApplyError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
