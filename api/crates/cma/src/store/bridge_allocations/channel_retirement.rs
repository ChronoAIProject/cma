//! Exact tree, human authority, cleanup receipt and canonical work share a fence.
mod audit;
mod authority;
mod lineage;
pub(crate) mod provider_finality;
mod save;
use super::{MongoBridgeAllocationStore, MutationError};
use crate::{
    bridge::provisioning::retirement::ChannelRetirementAuthority,
    sandbox::channel_retirement::model::{ChannelRetirementSnapshot, RetirementKey},
    store::{
        durability::majority_collection,
        sandbox_mongo::{commit_transaction, is_transient, start_transaction, CommitOutcome},
        MongoSandboxCredentialStore,
    },
};
use layer2_core::bridge::allocation::{AllocationError, AllocationRecord, AllocationState};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use std::time::SystemTime;

pub(crate) enum RetirementChange<'a> {
    Read,
    Begin,
    Stopped,
    Revoke(&'a RetirementKey),
    RuntimeIntent,
    RuntimeGone,
    ProviderFinality(
        &'a crate::sandbox::channel_retirement::provider_finality::ProviderFinalityReceipt,
    ),
    RetainStorage,
}
impl MongoBridgeAllocationStore {
    pub(crate) async fn change_channel_retirement(
        &self,
        permit: &ChannelRetirementAuthority,
        custody: &MongoSandboxCredentialStore,
        change: RetirementChange<'_>,
        now: SystemTime,
    ) -> Result<ChannelRetirementSnapshot, AllocationError> {
        for retry in 0..16 {
            if retry > 0 {
                tokio::task::yield_now().await;
            }
            permit.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = async {
                let (snapshot, document) =
                    authority::read_fenced(self, permit, &mut session).await?;
                let state = match &snapshot.state {
                    Some(state) => state.clone(),
                    None => snapshot.initial_state()?,
                };
                let expected = state
                    .keys
                    .iter()
                    .filter(|key| !key.revoked)
                    .map(|key| key.authority.clone())
                    .collect::<Vec<_>>();
                custody
                    .require_retirement_custody(
                        &self.records.namespace().db,
                        &snapshot.sandbox,
                        &expected,
                        &mut session,
                    )
                    .await
                    .map_err(custody_error)?;
                if matches!(change, RetirementChange::Read) {
                    return Ok(snapshot);
                }
                save::apply(
                    self,
                    permit,
                    custody,
                    snapshot,
                    &document,
                    &change,
                    now,
                    &mut session,
                )
                .await
            }
            .await;
            let snapshot = match result {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error) if is_transient(&error) => continue,
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            if let Err(error) = permit.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(snapshot),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable);
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}
fn custody_error(
    error: crate::store::sandbox_credentials::channel::ChannelCredentialError,
) -> MutationError {
    match error {
        crate::store::sandbox_credentials::channel::ChannelCredentialError::Corrupt => {
            AllocationError::Corrupt.into()
        }
        crate::store::sandbox_credentials::channel::ChannelCredentialError::Driver(error) => {
            MutationError::Mongo(error)
        }
    }
}
