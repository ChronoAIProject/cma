use crate::{
    cloud::Executor,
    model::{Build, BuildPhase, Error, Result},
    store::Store,
};

#[derive(Default)]
pub struct Worker {
    // Retain a known outcome across failed ticks. A process crash still requires
    // external reconciliation; this is not a durable provider outcome receipt.
    rejected: Option<Build>,
}

impl Worker {
    pub async fn tick(&mut self, store: &Store, executor: &Executor) -> Result<()> {
        self.settle_rejection(store).await?;
        // Only the process winning the durable queued->dispatching transition may
        // submit. Restarted workers observe dispatching operations without replay.
        if let Some(build) = store.claim_build().await? {
            match executor.submit(&build).await {
                Err(Error::BuildRejected) => {
                    self.rejected = Some(build);
                    self.settle_rejection(store).await?;
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

    async fn settle_rejection(&mut self, store: &Store) -> Result<()> {
        let Some(rejected) = self.rejected.as_mut() else {
            return Ok(());
        };
        for _ in 0..3 {
            if store.reject_dispatch(rejected).await? {
                self.rejected = None;
                return Ok(());
            }
            let current = store.build(&rejected.owner, &rejected.id).await?;
            if !same_request(rejected, &current) || current.revision < rejected.revision {
                return Err(Error::Conflict);
            }
            match current.phase {
                BuildPhase::Dispatching if current.cloud_id.is_none() => {
                    rejected.revision = current.revision;
                }
                BuildPhase::Running if current.cloud_id.is_some() => {
                    self.rejected = None;
                    return Ok(());
                }
                BuildPhase::Succeeded | BuildPhase::Failed | BuildPhase::Cancelled => {
                    self.rejected = None;
                    return Ok(());
                }
                _ => return Err(Error::Conflict),
            }
        }
        // Contention consumed this tick's budget, not the known outcome.
        Err(Error::Conflict)
    }
}

fn same_request(rejected: &Build, current: &Build) -> bool {
    current.id == rejected.id
        && current.owner == rejected.owner
        && current.image_id == rejected.image_id
        && current.request_key_hash == rejected.request_key_hash
        && current.request_sha256 == rejected.request_sha256
        && current.schema_version == rejected.schema_version
        && current.source == rejected.source
        && current.created_at_ms == rejected.created_at_ms
}
