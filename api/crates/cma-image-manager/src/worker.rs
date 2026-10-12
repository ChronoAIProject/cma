use crate::{
    cloud::Executor,
    model::{Error, Result},
    store::Store,
};

pub async fn tick(store: &Store, executor: &Executor) -> Result<()> {
    // Only the process winning the durable queued->dispatching transition may
    // submit. Restarted workers observe dispatching operations without replay.
    if let Some(build) = store.claim_build().await? {
        match executor.submit(&build).await {
            Err(Error::BuildRejected) => {
                store.reject_dispatch(&build).await?;
            }
            Err(_) => {
                tracing::warn!(build_id=%build.id,"build submission outcome unknown; reconciling original request")
            }
            Ok(()) => {}
        }
    }
    for build in store.pending().await? {
        match executor.observe(&build).await {
            Ok(Some(observation)) => {
                store
                    .observe_build(
                        &build,
                        &observation.id,
                        observation.phase,
                        observation.digest.as_deref(),
                    )
                    .await?;
            }
            Ok(None) => {
                store.defer_observation(&build).await?;
            }
            Err(_) => {
                store.defer_observation(&build).await?;
                tracing::warn!(build_id=%build.id,"build observation unavailable");
            }
        }
    }
    Ok(())
}
