//! Credential renewal writes retain the Ready allocation receipt and serialize
//! only its exact Sandbox, policy, current Bridge, and renewable human grant.
mod discovery;
mod provenance;
mod transaction;
use super::{MongoBridgeAllocationStore, MutationError};
use crate::{
    bridge::BridgeControlPlaneLease,
    sandbox::{
        channel_renewal::model::{ChannelRenewalSnapshot, RenewalAdmission},
        CredentialMaterial,
    },
    store::{
        sandbox_mongo::{commit_transaction, is_transient, start_transaction, CommitOutcome},
        MongoSandboxCredentialStore,
    },
};
use layer2_core::bridge::allocation::{
    AllocationAuthority, AllocationError, AllocationRecord, AllocationState,
};
use std::time::SystemTime;

enum Change<'a> {
    Read,
    Revalidate(&'a ChannelRenewalSnapshot),
    Admit {
        previous: &'a ChannelRenewalSnapshot,
        holder: &'a str,
        now: SystemTime,
    },
    Mint {
        previous: &'a ChannelRenewalSnapshot,
        custody: &'a MongoSandboxCredentialStore,
        material: &'a CredentialMaterial,
        now: SystemTime,
    },
    Deliver {
        previous: &'a ChannelRenewalSnapshot,
        now: SystemTime,
    },
    Revoke {
        previous: &'a ChannelRenewalSnapshot,
        custody: &'a MongoSandboxCredentialStore,
        now: SystemTime,
    },
}
impl Change<'_> {
    fn previous(&self) -> Option<&ChannelRenewalSnapshot> {
        match self {
            Self::Read => None,
            Self::Revalidate(previous)
            | Self::Admit { previous, .. }
            | Self::Mint { previous, .. }
            | Self::Deliver { previous, .. }
            | Self::Revoke { previous, .. } => Some(previous),
        }
    }
    fn require_claim(&self) -> Result<(), AllocationError> {
        if matches!(self, Self::Read | Self::Admit { .. }) {
            return Ok(());
        }
        let state = self
            .previous()
            .and_then(|previous| previous.renewal.as_ref())
            .ok_or(AllocationError::Corrupt)?;
        if state.claim.expires_at_ms <= now_ms(SystemTime::now())? {
            return Err(AllocationError::LeaseUnavailable);
        }
        Ok(())
    }
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn current_channel_renewal(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
    ) -> Result<ChannelRenewalSnapshot, AllocationError> {
        snapshot(
            self.renewal_transaction(authority, lease, allocation, Change::Read)
                .await?,
        )
    }
    pub(crate) async fn admit_channel_renewal(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelRenewalSnapshot,
        holder: &str,
        now: SystemTime,
    ) -> Result<RenewalAdmission, AllocationError> {
        self.renewal_transaction(
            authority,
            lease,
            &previous.allocation,
            Change::Admit {
                previous,
                holder,
                now,
            },
        )
        .await
    }
    pub(crate) async fn revalidate_channel_renewal(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelRenewalSnapshot,
    ) -> Result<(), AllocationError> {
        self.renewal_transaction(
            authority,
            lease,
            &previous.allocation,
            Change::Revalidate(previous),
        )
        .await
        .map(|_| ())
    }
    pub(crate) async fn commit_channel_renewal_mint(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelRenewalSnapshot,
        custody: &MongoSandboxCredentialStore,
        material: CredentialMaterial,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        self.renewal_transaction(
            authority,
            lease,
            &previous.allocation,
            Change::Mint {
                previous,
                custody,
                material: &material,
                now,
            },
        )
        .await
        .map(|_| ())
    }
    pub(crate) async fn commit_channel_renewal_delivery(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelRenewalSnapshot,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        self.renewal_transaction(
            authority,
            lease,
            &previous.allocation,
            Change::Deliver { previous, now },
        )
        .await
        .map(|_| ())
    }
    pub(crate) async fn commit_channel_renewal_revocation(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelRenewalSnapshot,
        custody: &MongoSandboxCredentialStore,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        self.renewal_transaction(
            authority,
            lease,
            &previous.allocation,
            Change::Revoke {
                previous,
                custody,
                now,
            },
        )
        .await
        .map(|_| ())
    }

    async fn renewal_transaction(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        allocation: &AllocationRecord,
        change: Change<'_>,
    ) -> Result<RenewalAdmission, AllocationError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    1_u64 << (attempt - 1).min(5),
                ))
                .await;
            }
            require_current(authority, lease, allocation)?;
            change.require_claim()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result =
                match transaction::apply(self, authority, lease, allocation, &change, &mut session)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = session.abort_transaction().await;
                        match error {
                            MutationError::Core(error) => return Err(error),
                            MutationError::Mongo(error) if is_transient(&error) => continue,
                            MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                        }
                    }
                };
            if let Err(error) =
                require_current(authority, lease, allocation).and_then(|_| change.require_claim())
            {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}

fn snapshot(admission: RenewalAdmission) -> Result<ChannelRenewalSnapshot, AllocationError> {
    match admission {
        RenewalAdmission::Resume(snapshot) | RenewalAdmission::Started(snapshot) => Ok(snapshot),
        _ => Err(AllocationError::Corrupt),
    }
}
fn require_current(
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
) -> Result<(), AllocationError> {
    authority.require_record(allocation)?;
    if !matches!(
        allocation.state,
        AllocationState::Ready | AllocationState::Retired
    ) || allocation.receipt.is_none()
        || !allocation.effect_started
        || allocation.owner != *lease.owner()
        || allocation.identity.bridge_id != *lease.bridge_id()
        || allocation.config.connector != *lease.connector()
        || tokio::time::Instant::now() >= lease.deadline()
    {
        return Err(AllocationError::AuthorityUnavailable);
    }
    Ok(())
}
fn now_ms(now: SystemTime) -> Result<i64, AllocationError> {
    now.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or(AllocationError::Unavailable)
}
