use super::*;
use context::ContextError;
use layer2_core::sandbox::Sandbox;

pub(super) async fn fence(
    store: &MongoSandboxStore,
    work: &SuggestionWork,
    parent: &Sandbox,
    session: &mut ClientSession,
) -> Result<(), ContextError> {
    let revision =
        i64::try_from(parent.revision).map_err(|error| ContextError::Domain(unavailable(error)))?;
    let result = crate::store::work_lease_update::with_fence(
        &store.sandboxes,
        doc! {
            "_id":parent.id.as_str(), "record.id":parent.id.as_str(),
            "document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "owner":work.owner.as_str(), "record.owner":work.owner.as_str(),
            "revision":revision, "record.revision":revision,
            "channel_reuse_cleanup_execution":{"$exists":false},
        },
        None,
        "suggestion_completion_fence",
        session,
    )
    .await
    .map_err(ContextError::Driver)?;
    if result.matched_count != 1 {
        return Err(ContextError::Domain(unavailable(
            "suggestion parent changed",
        )));
    }
    Ok(())
}
