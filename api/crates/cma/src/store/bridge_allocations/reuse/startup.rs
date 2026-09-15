//! Channel startup intent and real open receipts commit under one allocation,
//! exact human grant, resource lineage and reply-tool capability fence.
mod transaction;
use super::{
    super::{MongoBridgeAllocationStore, MutationError},
    transaction as reuse_transaction,
};
use crate::{
    bridge::BridgeControlPlaneLease,
    sandbox::{
        channel_reuse::{startup_model::ChannelReuseStartupContext, state::ChannelReuseSnapshot},
        channel_startup::model::ChannelStartupPlan,
    },
    store::{
        sandbox_mongo::{commit_transaction, is_transient, start_transaction, CommitOutcome},
        MongoSandboxStore,
    },
};
use layer2_core::bridge::allocation::{AllocationAuthority, AllocationError, AllocationRecord};
use layer3_adapter::WorktreeReceipt;

enum Change<'a> {
    Read,
    Prepare {
        previous: &'a ChannelReuseStartupContext,
        plan: &'a ChannelStartupPlan,
    },
    Finish {
        previous: &'a ChannelReuseStartupContext,
        plan: &'a ChannelStartupPlan,
        receipt: &'a WorktreeReceipt,
        conversation: &'a str,
        resources: &'a MongoSandboxStore,
    },
}

impl MongoBridgeAllocationStore {
    pub(crate) async fn reuse_startup_context(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelReuseSnapshot,
    ) -> Result<ChannelReuseStartupContext, AllocationError> {
        self.reuse_startup_transaction(authority, lease, snapshot, Change::Read)
            .await
    }

    pub(crate) async fn prepare_reuse_startup(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelReuseStartupContext,
        plan: &ChannelStartupPlan,
    ) -> Result<ChannelReuseStartupContext, AllocationError> {
        self.reuse_startup_transaction(
            authority,
            lease,
            &previous.snapshot,
            Change::Prepare { previous, plan },
        )
        .await
    }

    // Separate authority, open evidence and resource custody must agree at commit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn finish_reuse_startup(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        previous: &ChannelReuseStartupContext,
        plan: &ChannelStartupPlan,
        receipt: &WorktreeReceipt,
        conversation: &str,
        resources: &MongoSandboxStore,
    ) -> Result<AllocationRecord, AllocationError> {
        Ok(self
            .reuse_startup_transaction(
                authority,
                lease,
                &previous.snapshot,
                Change::Finish {
                    previous,
                    plan,
                    receipt,
                    conversation,
                    resources,
                },
            )
            .await?
            .snapshot
            .allocation)
    }

    async fn reuse_startup_transaction(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelReuseSnapshot,
        change: Change<'_>,
    ) -> Result<ChannelReuseStartupContext, AllocationError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    1_u64 << (attempt - 1).min(5),
                ))
                .await;
            }
            require_current(authority, lease, snapshot)?;
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
                match transaction::apply(self, authority, lease, snapshot, &change, &mut session)
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
            if let Err(error) = require_current(authority, lease, snapshot) {
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

fn require_current(
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    snapshot: &ChannelReuseSnapshot,
) -> Result<(), AllocationError> {
    authority.require_record(&snapshot.allocation)?;
    if tokio::time::Instant::now() >= lease.deadline()
        || snapshot.allocation.lease.as_ref().is_none_or(|claim| {
            claim.expires_at_ms <= mongodb::bson::DateTime::now().timestamp_millis()
        })
    {
        return Err(AllocationError::AuthorityUnavailable);
    }
    Ok(())
}
