use super::*;
use crate::config::bridge_flags::BridgeFlag;
use layer2_core::bridge::*;
use layer2_core::profiles::{AgentProfileRef, WorkspaceProfileRef};
use layer2_core::{AgentProfileId, SandboxId, WorkspaceId, WorkspaceProfileId};

struct RuntimeConnections {
    flags: BridgeFeatureFlags,
    inner_available: bool,
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for RuntimeConnections {
    fn capabilities(&self) -> BridgeCapabilityCatalog {
        let mut catalog = crate::bridge::capability_catalog::catalog(true);
        catalog.placements = crate::bridge::capability_catalog::shared_placements(
            Some(layer3_adapter::RuntimeContract::V3.capabilities()),
            self.flags,
        );
        catalog
    }
    fn check_new_placement(&self, _: &BridgeConfig) -> Result<(), BridgeError> {
        if self.inner_available {
            Ok(())
        } else {
            Err(BridgeError::Forbidden)
        }
    }
    async fn validate_connection(
        &self,
        _: &ResourceOwner,
        _: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::CustodyUnavailable,
        ))
    }
}

fn config(worktree: bool) -> BridgeConfig {
    let sandbox_id = SandboxId("sandbox-parent".into());
    let agent_profile = AgentProfileRef::new(AgentProfileId("agp-1".into()), 1);
    let shared_trust = BridgeSharedTrust::AllParticipantsShareFilesystemProcessesAndCredentials;
    let capacity = BridgeSharedCapacity {
        keep_other_workspaces: 1,
        keep_other_agents_per_workspace: 1,
    };
    BridgeConfig {
        display_name: "Shared selector fixture".into(),
        connector: BridgeConnector {
            issuer: "https://nyx.example".into(),
            base_url: "https://nyx.example".into(),
            bot_id: "bot".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["route".into()],
            key_id: "key".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/bridge/ingress".into(),
        },
        partition: BridgePartitionPolicy::ConversationSender,
        placement: if worktree {
            BridgePlacementPolicy::WorktreeAgentPerPartition {
                sandbox_id,
                workspace_id: WorkspaceId("workspace-parent".into()),
                agent_profile,
                shared_trust,
                capacity,
            }
        } else {
            BridgePlacementPolicy::WorkspacePerPartition {
                sandbox_id,
                workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp-1".into()), 1),
                agent_profile,
                shared_trust,
                capacity,
            }
        },
        admission: BridgeAdmissionPolicy::All,
        reply: BridgeReplyPolicy::FinalResponse,
        limits: BridgeLimits {
            max_partitions: 10,
            max_sandboxes: 10,
            max_queued_messages: 20,
            max_messages_per_minute: 60,
            max_payload_bytes: 4096,
            inbox_retention_seconds: 3600,
            reply_secret_retention_seconds: 600,
        },
    }
}

fn flags_without(disabled: Option<BridgeFlag>) -> BridgeFeatureFlags {
    BridgeFeatureFlags::from_lookup(&mut |key| {
        Ok(Some(
            (disabled.is_none_or(|flag| key != flag.key())).to_string(),
        ))
    })
    .unwrap()
}

#[test]
fn fresh_shared_selection_requires_its_mode_and_all_three_independent_stages() {
    for disabled in [
        None,
        Some(BridgeFlag::WorkspacePerPartition),
        Some(BridgeFlag::WorktreeAgentPerPartition),
        Some(BridgeFlag::Management),
        Some(BridgeFlag::Allocation),
        Some(BridgeFlag::Execution),
        Some(BridgeFlag::Telegram),
        Some(BridgeFlag::Ingress),
        Some(BridgeFlag::Outbound),
    ] {
        let flags = flags_without(disabled);
        let connection = configured(
            Arc::new(RuntimeConnections {
                flags,
                inner_available: true,
            }),
            flags,
        );
        for worktree in [false, true] {
            let expected = disabled.is_none_or(|flag| match flag {
                BridgeFlag::WorkspacePerPartition => worktree,
                BridgeFlag::WorktreeAgentPerPartition => !worktree,
                BridgeFlag::Ingress | BridgeFlag::Outbound => true,
                _ => false,
            });
            assert_eq!(
                connection.check_new_placement(&config(worktree)),
                if expected {
                    Ok(())
                } else {
                    Err(BridgeError::Unavailable)
                }
            );
        }
        assert!(connection
            .capabilities()
            .connectors
            .iter()
            .all(|entry| !entry.live_verified));
    }
    let flags = flags_without(None);
    let missing = configured(Arc::new(crate::bridge::UnavailableConnections), flags);
    assert_eq!(
        missing.check_new_placement(&config(false)),
        Err(BridgeError::Unavailable)
    );
    let denied = configured(
        Arc::new(RuntimeConnections {
            flags,
            inner_available: false,
        }),
        flags,
    );
    assert_eq!(
        denied.check_new_placement(&config(true)),
        Err(BridgeError::Forbidden)
    );
}

#[tokio::test]
async fn disabled_shared_selectors_do_not_replace_retained_connection_authority() {
    let flags = BridgeFeatureFlags::default();
    let connection = configured(
        Arc::new(RuntimeConnections {
            flags,
            inner_available: true,
        }),
        flags,
    );
    let owner = ResourceOwner::Person {
        subject: layer2_core::SubjectId::new("owner"),
    };
    for worktree in [false, true] {
        assert_eq!(
            connection.check_new_placement(&config(worktree)),
            Err(BridgeError::Unavailable)
        );
        assert!(matches!(
            connection
                .validate_connection(&owner, &config(worktree))
                .await,
            Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::CustodyUnavailable
            ))
        ));
    }
}
