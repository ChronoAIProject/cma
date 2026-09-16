use super::*;
use crate::{
    bridge::provisioning::retirement::ChannelRetirementAuthority,
    sandbox::channel_retirement::model::{ChannelRetirementSnapshot, RetirementPhase},
    store::bridge_allocations::channel_retirement::RetirementChange as Change,
};
use layer2_core::{bridge::allocation::AllocationError, sandbox::SandboxWorkStep};
use layer3_adapter::RuntimeHandle;

pub(super) enum RetirementStep {
    Advanced,
    Retained(Box<super::assessment::FinalRetentionAssessment>),
}

impl ChannelRetirementDriver {
    async fn read(
        &self,
        permit: &ChannelRetirementAuthority,
        now: SystemTime,
    ) -> Result<ChannelRetirementSnapshot, AllocationError> {
        self.credentials
            .revalidate_control_plane(permit.human())
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.allocations
            .change_channel_retirement(permit, &self.custody, Change::Read, now)
            .await
    }
    async fn commit(
        &self,
        permit: &ChannelRetirementAuthority,
        change: Change<'_>,
        now: SystemTime,
    ) -> Result<RetirementStep, AllocationError> {
        self.credentials
            .revalidate_control_plane(permit.human())
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.allocations
            .change_channel_retirement(permit, &self.custody, change, now)
            .await
            .map(|_| RetirementStep::Advanced)
    }
    pub(super) async fn run_step(
        &self,
        sandbox: &Sandbox,
        agents: &dyn super::super::worker::LiveAgentStopper,
        now: SystemTime,
    ) -> Result<RetirementStep, AllocationError> {
        let permit = self
            .authority
            .resolve(sandbox)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let snapshot = self.read(&permit, now).await?;
        if snapshot.state.is_none() {
            return self.commit(&permit, Change::Begin, now).await;
        }
        let state = snapshot.state.as_ref().ok_or(AllocationError::Corrupt)?;
        match (sandbox.work.step, state.phase) {
            (SandboxWorkStep::StopAgents, RetirementPhase::StopIntent) => {
                let runtime = sandbox
                    .runtime
                    .as_ref()
                    .map(|binding| RuntimeHandle(binding.runtime_id.clone()));
                agents
                    .stop_live_agents(
                        sandbox,
                        runtime.as_ref(),
                        &format!("{}:retirement", snapshot.allocation.operation_id),
                    )
                    .await
                    .map_err(|_| AllocationError::Unavailable)?;
                self.commit(&permit, Change::Stopped, now).await
            }
            (SandboxWorkStep::RevokeRetirementCredentials, RetirementPhase::Revoke) => {
                let key = state
                    .keys
                    .iter()
                    .find(|key| !key.revoked)
                    .ok_or(AllocationError::Corrupt)?;
                self.read(&permit, now).await?;
                self.issuer
                    .revoke_bridge_sandbox_key(permit.human(), &key.credential_id, &key.authority)
                    .await
                    .map_err(|_| AllocationError::AuthorityUnavailable)?;
                self.commit(&permit, Change::Revoke(key), now).await
            }
            (SandboxWorkStep::TeardownRuntime, RetirementPhase::CredentialsCleared) => {
                self.commit(&permit, Change::RuntimeIntent, now).await
            }
            (SandboxWorkStep::TeardownRuntime, RetirementPhase::RuntimeIntent) => {
                self.teardown(&permit, &snapshot, now).await?;
                self.commit(&permit, Change::RuntimeGone, now).await
            }
            (SandboxWorkStep::MarkRetired, RetirementPhase::RuntimeGone)
                if snapshot.retention.is_none() =>
            {
                self.commit(&permit, Change::RetainStorage, now).await
            }
            (SandboxWorkStep::MarkRetired, RetirementPhase::RuntimeGone) => {
                Ok(RetirementStep::Retained(Box::new(
                    super::assessment::FinalRetentionAssessment::capture(&snapshot)?,
                )))
            }
            // Home, checkpoint and history retention require their own exact
            // ownership gates. Never fall through to the generic root purger.
            _ => Err(AllocationError::AuthorityUnavailable),
        }
    }
    async fn teardown(
        &self,
        permit: &ChannelRetirementAuthority,
        snapshot: &ChannelRetirementSnapshot,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        let binding = snapshot
            .sandbox
            .runtime
            .as_ref()
            .ok_or(AllocationError::AuthorityUnavailable)?;
        self.read(permit, now).await?;
        let adopted = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.provisioning.settings.deployment_id,
        )
        .await;
        let handle = match adopted {
            Ok(handle) => handle,
            Err(_) => return self.require_absent(binding).await,
        };
        self.read(permit, now).await?;
        if self.adapter.teardown(&handle).await.is_err() {
            return self.require_absent(binding).await;
        }
        Ok(())
    }
    async fn require_absent(
        &self,
        binding: &layer2_core::sandbox::RuntimeBinding,
    ) -> Result<(), AllocationError> {
        let rows = self
            .adapter
            .runtime_inventory()
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let mut ids = std::collections::BTreeSet::new();
        if rows.len() > 1000
            || rows
                .iter()
                .any(|row| row.handle.0 == binding.runtime_id || !ids.insert(&row.handle.0))
        {
            return Err(AllocationError::Unavailable);
        }
        Ok(())
    }
}
