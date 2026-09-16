use std::sync::Arc;

use crate::{
    bridge::provisioning::{
        revival::{ChannelRevivalAuthoritySource, ManagedChannelRevivalAuthority},
        BridgeProvisioningAuthority,
    },
    store::{MongoBridgeAllocationStore, MongoBridgeInvokeStore},
};
use layer2_core::sandbox::{
    agent_io::{AdmittedAgentCommand, AgentCommand, ChannelExecutionAuthorization},
    SandboxStatus,
};

pub(crate) struct ChannelCommandRevival {
    pub authority: Arc<dyn ChannelRevivalAuthoritySource>,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub invokes: Arc<MongoBridgeInvokeStore>,
}

impl ChannelCommandRevival {
    pub(crate) fn configured(
        bridge: &crate::bridge::composition::BridgeComposition,
        definitions: &layer2_core::bridge::BridgeService,
        credentials: Option<Arc<crate::bridge::ManagedBridgeCredentials>>,
    ) -> Option<Arc<Self>> {
        let credentials = credentials?;
        let dependencies = credentials.provisioning.clone()?;
        Some(Arc::new(Self {
            authority: Arc::new(ManagedChannelRevivalAuthority {
                definitions: definitions.clone(),
                allocations: bridge.allocations.clone(),
                authority: Arc::new(BridgeProvisioningAuthority::new(credentials, dependencies)),
            }),
            allocations: bridge.allocations.clone(),
            invokes: bridge.invoke_store.clone(),
        }))
    }

    pub(crate) async fn prepare(
        &self,
        command: &AdmittedAgentCommand,
        channel: &ChannelExecutionAuthorization,
    ) {
        let fence = channel.fence();
        let sandbox = &fence.proof().resources().lineage.sandbox;
        if sandbox.status != SandboxStatus::Paused
            || !matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
            || command.validate().is_err()
            || !command.awaiting_claim(&layer2_core::now_rfc3339())
            || command.queue_expired(&layer2_core::now_rfc3339())
        {
            return;
        }
        let _ = tokio::time::timeout_at(fence.proof().deadline(), async {
            if matches!(
                fence.source().allocation().placement,
                layer2_core::bridge::allocation::AllocationPlacement::ReuseParent { .. }
            ) {
                let permit = self
                    .authority
                    .resolve_reused(fence.source().allocation(), sandbox)
                    .await
                    .ok()?;
                return self
                    .invokes
                    .schedule_reused_channel_command_revival(
                        &self.allocations,
                        &permit,
                        &fence,
                        command,
                    )
                    .await
                    .ok();
            }
            // Invoke permission and original human lifecycle permission are
            // independently required and fenced in the same scheduling write.
            let permit = self.authority.resolve(sandbox).await.ok()?;
            self.invokes
                .schedule_channel_command_revival(&self.allocations, &permit, &fence, command)
                .await
                .ok()
        })
        .await;
    }
}
