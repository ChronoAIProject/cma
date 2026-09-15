//! Exact fresh resource admission under allocation authority. This deliberately
//! does not use warm adoption or schedule the ordinary provisioning worker.
mod transaction;

use std::time::Duration;

use layer2_core::bridge::allocation::{
    AllocationError, ChannelSandboxAdmission, ChannelSandboxAdmissionStore,
    ChannelSandboxReservation,
};
use mongodb::{bson::Document, Collection};

use crate::store::{
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    },
};

use super::{MongoBridgeAllocationStore, MutationError};

const ADMISSION_TRANSACTION_ATTEMPTS: usize = 16;

struct Resources {
    sandboxes: Collection<Document>,
    events: Collection<Document>,
    admissions: Collection<Document>,
    policies: Collection<Document>,
}

impl Resources {
    fn new(store: &MongoBridgeAllocationStore) -> Result<Self, AllocationError> {
        let namespace = store.records.namespace();
        if namespace.db != store.bridges.definitions.namespace().db
            || namespace.db != store.heads.namespace().db
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let database = store.bridges.client.database(&namespace.db);
        Ok(Self {
            sandboxes: majority_collection(&database, "sandboxes"),
            events: majority_collection(&database, "sandbox_events"),
            admissions: majority_collection(&database, "sandbox_admissions"),
            policies: majority_collection(&database, "resource_policies"),
        })
    }
}

#[async_trait::async_trait]
impl ChannelSandboxAdmissionStore for MongoBridgeAllocationStore {
    async fn admit_channel_sandbox(
        &self,
        request: ChannelSandboxReservation,
    ) -> Result<ChannelSandboxAdmission, AllocationError> {
        let resources = Resources::new(self)?;
        for attempt in 0..ADMISSION_TRANSACTION_ATTEMPTS {
            // These transactions share the bridge, allocation and owner write
            // fences. Yield-only retries can exhaust before a winner commits.
            // Match ordinary Sandbox admission's bounded 1–32ms backoff.
            if attempt > 0 {
                let delay_ms = 1_u64 << (attempt - 1).min(5);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            // Waiting never extends or bypasses the original authority expiry.
            request.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = match transaction::apply(self, &resources, &request, &mut session).await {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue;
                        }
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            if let Err(error) = request.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                // A fresh proof retries this exact resource identity. Unknown
                // commit never means the resource was not admitted.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}
