use std::collections::VecDeque;

use crate::{
    cloud::Executor,
    model::{Build, BuildPhase, Error, Result},
    store::Store,
};

const REJECTIONS_PER_TICK: usize = 8;

#[derive(Default)]
pub struct Worker {
    // Retain a known outcome across failed ticks. A process crash still requires
    // external reconciliation; this is not a durable provider outcome receipt.
    rejected: VecDeque<Build>,
}

impl Worker {
    pub async fn tick(&mut self, store: &Store, executor: &Executor) -> Result<()> {
        let mut rejection_error = None;
        for _ in 0..self.rejected.len().min(REJECTIONS_PER_TICK) {
            let mut rejected = self.rejected.pop_front().expect("bounded by queue length");
            if let Err(error) = settle_rejection(store, &mut rejected).await {
                self.rejected.push_back(rejected);
                rejection_error.get_or_insert(error);
            }
        }
        let mut excluded: Vec<_> = self.rejected.iter().map(|build| build.id.clone()).collect();
        // Only the process winning the durable queued->dispatching transition may
        // submit. Restarted workers observe dispatching operations without replay.
        if let Some(build) = store.claim_build_excluding(&excluded).await? {
            match executor.submit(&build).await {
                Err(Error::BuildRejected) => {
                    let mut rejected = build;
                    if let Err(error) = settle_rejection(store, &mut rejected).await {
                        excluded.push(rejected.id.clone());
                        self.rejected.push_back(rejected);
                        rejection_error.get_or_insert(error);
                    }
                }
                Err(_) => {
                    tracing::warn!(build_id=%build.id,"build submission outcome unknown; reconciling original request")
                }
                Ok(()) => {}
            }
        }
        for build in store.pending_excluding(&excluded).await? {
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
        rejection_error.map_or(Ok(()), Err)
    }
}

async fn settle_rejection(store: &Store, rejected: &mut Build) -> Result<()> {
    for _ in 0..3 {
        if store.reject_dispatch(rejected).await? {
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
            BuildPhase::Running if current.cloud_id.is_some() => return Ok(()),
            BuildPhase::Succeeded | BuildPhase::Failed | BuildPhase::Cancelled => return Ok(()),
            _ => return Err(Error::Conflict),
        }
    }
    Err(Error::Conflict)
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
