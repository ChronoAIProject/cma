//! Existing Channel runtime authority continues independently of the rollout
//! gates for new Channel admission and new allocation release.
use super::{
    channel_scheduler::{ChannelWorkError as Error, ChannelWorkPort},
    provisioning::BridgeProvisioningAuthority,
};
use crate::{
    sandbox::channel_renewal::ChannelCredentialRenewalDriver, store::MongoBridgeAllocationStore,
};
use layer2_core::bridge::{allocation::AllocationError, BridgeService};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

pub(crate) struct ChannelRenewalWork {
    pub definitions: BridgeService,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<BridgeProvisioningAuthority>,
    pub driver: ChannelCredentialRenewalDriver,
}
#[async_trait::async_trait]
impl ChannelWorkPort for ChannelRenewalWork {
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, Error> {
        self.allocations
            .due_channel_renewal_sandboxes(after, SystemTime::now())
            .await
            .map_err(classify)
    }
    async fn run_one(&self, sandbox_id: &str) -> Result<(), Error> {
        let result = self.advance(sandbox_id).await;
        if let Err(error) = result {
            let (delay, state) = match error {
                Error::Suspended => (Duration::from_secs(30), "authority_required"),
                Error::Changed => (Duration::from_secs(1), "changed"),
                Error::Unavailable => (Duration::from_secs(5), "unavailable"),
                Error::Corrupt => (Duration::from_secs(30), "inconsistent"),
                Error::Unsupported => (Duration::from_secs(30), "unsupported"),
            };
            self.allocations
                .defer_channel_renewal(sandbox_id, delay, state)
                .await
                .map_err(classify)?;
        }
        result
    }
}
impl ChannelRenewalWork {
    async fn advance(&self, sandbox_id: &str) -> Result<(), Error> {
        let Some(record) = self
            .allocations
            .due_channel_renewal_record(sandbox_id, SystemTime::now())
            .await
            .map_err(classify)?
        else {
            return Ok(());
        };
        let (authority, lease) = self
            .authority
            .resolve(&self.definitions, &record)
            .await
            .map_err(|_| Error::Suspended)?;
        self.driver
            .run_one(&record, &authority, &lease, SystemTime::now())
            .await
            .map_err(classify)
    }
}
fn classify(error: AllocationError) -> Error {
    match error {
        AllocationError::AuthorityUnavailable | AllocationError::InvalidProfiles => {
            Error::Suspended
        }
        AllocationError::Conflict | AllocationError::LeaseUnavailable => Error::Changed,
        AllocationError::Corrupt | AllocationError::IdentityMismatch => Error::Corrupt,
        _ => Error::Unavailable,
    }
}
