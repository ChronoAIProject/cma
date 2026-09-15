use super::*;
use crate::sandbox::{
    channel_children::{self, ChannelChildEvidence},
    channel_provisioning::state::ChannelProvisioningPhase,
    channel_startup::ChannelStartupDriver,
    OrnnAgentSkillResolver,
};

impl Fixture {
    pub async fn agent_start(&mut self) -> ChannelProvisioningSnapshot {
        for _ in 0..5 {
            let snapshot = self.children().await;
            let handle = layer3_adapter::RuntimeHandle::new(
                &snapshot.sandbox.runtime.as_ref().unwrap().runtime_id,
            );
            let evidence = match snapshot.state.phase {
                ChannelProvisioningPhase::WorkspaceAdmission => {
                    ChannelChildEvidence::WorkspaceAdmission
                }
                ChannelProvisioningPhase::WorkspacePrepare => {
                    let workspace = self.allocations.channel_workspace(&snapshot).await.unwrap();
                    let request = channel_children::prepare_request(&snapshot, &workspace);
                    let receipt = self
                        .adapter
                        .prepare_workspace(&handle, &request)
                        .await
                        .unwrap();
                    ChannelChildEvidence::WorkspacePrepare { request, receipt }
                }
                ChannelProvisioningPhase::WorkspaceSetup => {
                    let workspace = self.allocations.channel_workspace(&snapshot).await.unwrap();
                    let request = channel_children::setup_request(&snapshot, &workspace);
                    let receipt = self
                        .adapter
                        .run_workspace_setup(&handle, &request)
                        .await
                        .unwrap();
                    ChannelChildEvidence::WorkspaceSetup { request, receipt }
                }
                ChannelProvisioningPhase::AgentAdmission => ChannelChildEvidence::AgentAdmission {
                    capabilities: crate::sandbox::runtime_capabilities(self.adapter.capabilities()),
                },
                ChannelProvisioningPhase::AgentStart => return snapshot,
                _ => panic!("unexpected Channel child phase"),
            };
            self.record = self
                .allocations
                .commit_channel_child(&self.authority, &self.lease, &snapshot, &evidence)
                .await
                .unwrap()
                .allocation;
        }
        panic!("did not reach real Agent startup");
    }

    pub fn startup_driver(&self) -> ChannelStartupDriver {
        ChannelStartupDriver {
            allocations: self.allocations.clone(),
            resources: self.resources.clone(),
            credentials: Arc::new(VerifiedHuman),
            adapter: self.adapter.clone(),
            skills: OrnnAgentSkillResolver::anonymous("https://nyx.example").unwrap(),
        }
    }

    pub async fn ready(&mut self) -> AllocationRecord {
        let snapshot = self.agent_start().await;
        self.record = self
            .startup_driver()
            .advance(&self.authority, &self.lease, &snapshot)
            .await
            .unwrap();
        self.record.clone()
    }
}
