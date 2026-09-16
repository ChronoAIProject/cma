use super::*;

struct PlacementValidator {
    selected: AtomicBool,
    inner: Arc<Validator>,
}

#[async_trait::async_trait]
impl BridgeConnectionValidator for PlacementValidator {
    fn check_new_placement(&self, config: &BridgeConfig) -> Result<(), BridgeError> {
        if config.placement.reused_parent().is_some() && !self.selected.load(Ordering::SeqCst) {
            return Err(BridgeError::Unavailable);
        }
        Ok(())
    }

    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        self.inner.validate_connection(owner, config).await
    }
}

fn fixture() -> (Fixture, Arc<PlacementValidator>) {
    let mut fixture = Fixture::new();
    let validator = Arc::new(PlacementValidator {
        selected: AtomicBool::new(false),
        inner: fixture.validator.clone(),
    });
    fixture.service = BridgeService::new(
        fixture.store.clone(),
        fixture.organizations.clone(),
        validator.clone(),
    );
    (fixture, validator)
}

fn shared(worktree: bool) -> BridgeConfig {
    let mut value = config();
    let sandbox_id = crate::SandboxId("sandbox-parent".into());
    let agent_profile = AgentProfileRef::new(AgentProfileId("agp-1".into()), 3);
    let shared_trust = BridgeSharedTrust::AllParticipantsShareFilesystemProcessesAndCredentials;
    let capacity = BridgeSharedCapacity {
        keep_other_workspaces: 1,
        keep_other_agents_per_workspace: 1,
    };
    value.placement = if worktree {
        BridgePlacementPolicy::WorktreeAgentPerPartition {
            sandbox_id,
            workspace_id: crate::WorkspaceId("workspace-parent".into()),
            agent_profile,
            shared_trust,
            capacity,
        }
    } else {
        BridgePlacementPolicy::WorkspacePerPartition {
            sandbox_id,
            workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp-1".into()), 2),
            agent_profile,
            shared_trust,
            capacity,
        }
    };
    value
}

#[tokio::test]
async fn shared_placement_gate_preserves_receipts_settings_and_safe_reductions() {
    for worktree in [false, true] {
        let (fixture, selector) = fixture();
        let original = shared(worktree);
        let create = BridgeChange::Create {
            owner: owner(),
            config: Box::new(original.clone()),
        };
        assert_eq!(
            fixture
                .change(0, "create-shared", create.clone())
                .await
                .unwrap_err(),
            BridgeError::Unavailable
        );
        assert!(fixture.store.bridge(&fixture.id).await.unwrap().is_none());
        selector.selected.store(true, Ordering::SeqCst);
        let receipt = fixture
            .change(0, "create-shared", create.clone())
            .await
            .unwrap();
        selector.selected.store(false, Ordering::SeqCst);
        assert_eq!(
            fixture.change(0, "create-shared", create).await.unwrap(),
            receipt
        );
        assert_eq!(
            fixture
                .change(1, "enable-shared", BridgeChange::Enable)
                .await
                .unwrap_err(),
            BridgeError::Unavailable
        );
        let mut renamed = original.clone();
        renamed.display_name = "Retained placement".into();
        let renamed_receipt = fixture
            .change(
                1,
                "rename-shared",
                BridgeChange::Update {
                    config: Box::new(renamed.clone()),
                    previous_work: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed_receipt.revision, 2);
        if let BridgePlacementPolicy::WorkspacePerPartition { sandbox_id, .. }
        | BridgePlacementPolicy::WorktreeAgentPerPartition { sandbox_id, .. } =
            &mut renamed.placement
        {
            *sandbox_id = crate::SandboxId("different-parent".into());
        }
        assert_eq!(
            fixture
                .change(
                    2,
                    "change-parent",
                    BridgeChange::Update {
                        config: Box::new(renamed),
                        previous_work: None,
                    }
                )
                .await
                .unwrap_err(),
            BridgeError::Unavailable
        );
        assert!(
            fixture
                .service
                .detail(&context("human"), Some("fixture-auth"), &fixture.id)
                .await
                .unwrap()
                .can_manage
        );
        assert_eq!(
            fixture
                .change(
                    2,
                    "stop-shared",
                    BridgeChange::Emergency {
                        controls: BridgeEmergencyControls {
                            ingress_stopped: true,
                            execution_stopped: true,
                            outbound_stopped: true,
                        },
                    }
                )
                .await
                .unwrap()
                .revision,
            3
        );
        assert_eq!(
            fixture
                .store
                .bridge(&fixture.id)
                .await
                .unwrap()
                .unwrap()
                .current()
                .unwrap()
                .config
                .placement,
            original.placement
        );
    }
}

#[tokio::test]
async fn fresh_shared_change_is_gated_but_unshared_configuration_and_validation_still_work() {
    let (fixture, selector) = fixture();
    fixture.create(owner()).await;
    let change = BridgeChange::Update {
        config: Box::new(shared(false)),
        previous_work: None,
    };
    assert_eq!(
        fixture
            .change(1, "select-shared", change.clone())
            .await
            .unwrap_err(),
        BridgeError::Unavailable
    );
    selector.selected.store(true, Ordering::SeqCst);
    let receipt = fixture
        .change(1, "select-shared", change.clone())
        .await
        .unwrap();
    selector.selected.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture.change(1, "select-shared", change).await.unwrap(),
        receipt
    );
    // Retained connection validation continues to use its ordinary authority:
    // this fixture lacks a shared target observation, yielding that exact error.
    assert!(matches!(
        selector.validate_connection(&owner(), &shared(false)).await,
        Err(BridgeError::ConnectionRequired(_))
    ));
    assert!(selector
        .validate_connection(&owner(), &config())
        .await
        .is_ok());
    assert_eq!(
        fixture
            .change(
                2,
                "leave-shared",
                BridgeChange::Update {
                    config: Box::new(config()),
                    previous_work: None,
                }
            )
            .await
            .unwrap()
            .revision,
        3
    );
}
