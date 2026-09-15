use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use crate::resource_access::{
    CallerContext, CredentialContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    OrganizationMembership, OrganizationRole, ResourceActor, ResourceOwner,
};
use crate::{
    AgentProfileId, Authorization, Caller, Capability, SandboxProfileId, SubjectId,
    WorkspaceProfileId,
};

use super::*;

fn owner() -> ResourceOwner {
    ResourceOwner::Person {
        subject: SubjectId::new("human"),
    }
}
fn organization() -> ResourceOwner {
    ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    }
}

fn context(subject: &str) -> CallerContext {
    let subject = SubjectId::new(subject);
    CallerContext {
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://nyx.example".into(),
            subject: subject.clone(),
            credential_id: "login-1".into(),
        },
        actor: ResourceActor::Human { subject },
        memberships: vec![],
    }
}

fn config() -> BridgeConfig {
    BridgeConfig {
        display_name: "Disposable bridge".into(),
        connector: BridgeConnector {
            issuer: "https://nyx.example".into(),
            base_url: "https://nyx.example".into(),
            bot_id: "existing-bot".into(),
            platform: BridgePlatform::Telegram,
            route_ids: vec!["route-1".into()],
            key_id: "key-1".into(),
            custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
            callback_url: "https://cma.example/bridge/ingress".into(),
        },
        partition: BridgePartitionPolicy::ConversationSender,
        placement: BridgePlacementPolicy::NewSandbox {
            sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp-1".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(WorkspaceProfileId("wkp-1".into()), 2),
            agent_profile: AgentProfileRef::new(AgentProfileId("agp-1".into()), 3),
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

fn observation(owner: &ResourceOwner, config: &BridgeConfig) -> BridgeConnectionObservation {
    BridgeConnectionObservation {
        connector: config.connector.clone(),
        bot_owner: owner.clone(),
        key_owner: owner.clone(),
        custody_owner: owner.clone(),
        custody_revision: 1,
        key_state_version: 3,
        routes: config
            .connector
            .route_ids
            .iter()
            .map(|id| BridgeRouteObservation {
                route_id: id.clone(),
                owner: owner.clone(),
                bot_id: config.connector.bot_id.clone(),
                key_id: config.connector.key_id.clone(),
                enabled: true,
            })
            .collect(),
        key_active: true,
        custody_available: true,
        callback_conflict: false,
        renewable_sandbox_authority: true,
        exact_profiles_available: true,
        existing_target: None,
        deadline: tokio::time::Instant::now() + Duration::from_secs(30),
    }
}

struct Validator {
    available: AtomicBool,
    delay: AtomicU64,
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Validator {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        if !self.available.load(Ordering::SeqCst) {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::RenewableAuthorityUnavailable,
            ));
        }
        tokio::time::sleep(Duration::from_secs(self.delay.load(Ordering::SeqCst))).await;
        BridgeValidation::from_observation(owner, config, observation(owner, config))
    }
}

struct Organizations {
    lifetime: AtomicU64,
    role: std::sync::Mutex<Option<OrganizationRole>>,
    outage: AtomicBool,
}
#[async_trait::async_trait]
impl OrganizationAuthority for Organizations {
    async fn organization_context(
        &self,
        caller: &Caller,
        _credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if self.outage.load(Ordering::SeqCst) {
            return Err(MembershipError::Unavailable);
        }
        let role = self
            .role
            .lock()
            .unwrap()
            .ok_or(MembershipError::NotMember)?;
        let mut current = context(caller.subject.as_str());
        current.memberships.push(
            OrganizationMembership::new(
                current.credential.clone(),
                org.clone(),
                role,
                SystemTime::now(),
                SystemTime::now() + Duration::from_secs(self.lifetime.load(Ordering::SeqCst)),
            )
            .unwrap(),
        );
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}

struct Fixture {
    service: BridgeService,
    store: Arc<MemoryBridgeStore>,
    organizations: Arc<Organizations>,
    validator: Arc<Validator>,
    id: BridgeId,
}
impl Fixture {
    fn new() -> Self {
        let store = Arc::new(MemoryBridgeStore::default());
        let organizations = Arc::new(Organizations {
            lifetime: AtomicU64::new(60),
            role: std::sync::Mutex::new(Some(OrganizationRole::Admin)),
            outage: AtomicBool::new(false),
        });
        let validator = Arc::new(Validator {
            delay: AtomicU64::new(0),
            available: AtomicBool::new(true),
        });
        let service = BridgeService::new(store.clone(), organizations.clone(), validator.clone());
        let id = BridgeService::creation_id(&context("human"), "create-1").unwrap();
        Self {
            service,
            store,
            organizations,
            validator,
            id,
        }
    }
    async fn change(
        &self,
        revision: u64,
        key: &str,
        change: BridgeChange,
    ) -> Result<BridgeReceipt, BridgeError> {
        self.service
            .mutate(
                &context("human"),
                Some("fixture-auth"),
                BridgeMutation {
                    id: self.id.clone(),
                    expected_revision: revision,
                    idempotency_key: key.into(),
                    change,
                },
            )
            .await
    }
    async fn create(&self, owner: ResourceOwner) -> BridgeReceipt {
        self.change(
            0,
            "create-1",
            BridgeChange::Create {
                owner,
                config: Box::new(config()),
            },
        )
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn original_identity_and_secrets_never_enter_member_projection_or_receipt() {
    let f = Fixture::new();
    let receipt = f.create(organization()).await;
    assert_eq!(receipt.owner, organization());
    assert_eq!(receipt.actor, context("human").actor);
    *f.organizations.role.lock().unwrap() = Some(OrganizationRole::Member);
    let view = f
        .service
        .detail(&context("human"), Some("fixture-auth"), &f.id)
        .await
        .unwrap();
    assert!(!view.can_manage);
    let json = serde_json::to_string(&(view, receipt)).unwrap();
    for forbidden in [
        "key-1",
        "brsec_00000000000000000000000000000001",
        "route-1",
        "bridge/ingress",
        "secret_reference",
    ] {
        assert!(!json.contains(forbidden));
    }
    assert!(!format!("{:?}", config()).contains("brsec_00000000000000000000000000000001"));
    assert_eq!(
        f.change(1, "enable", BridgeChange::Enable).await,
        Err(BridgeError::Forbidden)
    );
}

#[tokio::test]
async fn current_role_outage_and_nonhuman_actor_fail_closed_without_personal_outage() {
    let f = Fixture::new();
    f.create(organization()).await;
    *f.organizations.role.lock().unwrap() = None;
    assert_eq!(
        f.change(1, "enable", BridgeChange::Enable).await,
        Err(BridgeError::NotFound)
    );
    f.organizations.outage.store(true, Ordering::SeqCst);
    assert_eq!(
        f.change(1, "enable", BridgeChange::Enable).await,
        Err(BridgeError::AuthorityUnavailable)
    );
    let personal = Fixture::new();
    personal.organizations.outage.store(true, Ordering::SeqCst);
    personal.create(owner()).await;
    let mut caller = context("human");
    caller.actor = ResourceActor::Channel {
        bridge_id: "brg-1".into(),
        grant_id: "grant-1".into(),
        platform: "telegram".into(),
        chat_id: "chat-1".into(),
        sender_id: "human".into(),
    };
    assert_eq!(
        personal.service.detail(&caller, None, &personal.id).await,
        Err(BridgeError::NotFound)
    );
    assert_eq!(
        personal
            .service
            .detail(&context("unrelated"), None, &personal.id)
            .await,
        Err(BridgeError::NotFound)
    );
}

#[tokio::test]
async fn enable_requires_current_renewable_connection_but_disabled_draft_is_recoverable() {
    let f = Fixture::new();
    f.validator.available.store(false, Ordering::SeqCst);
    f.create(owner()).await;
    assert_eq!(
        f.change(1, "enable", BridgeChange::Enable).await,
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::RenewableAuthorityUnavailable
        ))
    );
    assert_eq!(f.store.bridge(&f.id).await.unwrap().unwrap().revision, 1);
    f.validator.available.store(true, Ordering::SeqCst);
    f.change(1, "enable", BridgeChange::Enable).await.unwrap();
}

#[tokio::test]
async fn idempotency_conflict_cas_race_and_revoked_replay_preserve_one_transition() {
    let f = Fixture::new();
    let created = f.create(organization()).await;
    assert_eq!(
        f.change(
            0,
            "create-1",
            BridgeChange::Create {
                owner: organization(),
                config: Box::new(config())
            }
        )
        .await
        .unwrap(),
        created
    );
    assert_eq!(
        f.change(1, "create-1", BridgeChange::Enable).await,
        Err(BridgeError::IdempotencyConflict)
    );
    let (a, b) = tokio::join!(
        f.change(1, "enable-a", BridgeChange::Enable),
        f.change(1, "enable-b", BridgeChange::Enable)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(a == Err(BridgeError::Conflict) || b == Err(BridgeError::Conflict));
    *f.organizations.role.lock().unwrap() = Some(OrganizationRole::Viewer);
    assert_eq!(
        f.change(
            0,
            "create-1",
            BridgeChange::Create {
                owner: organization(),
                config: Box::new(config())
            }
        )
        .await,
        Err(BridgeError::Forbidden)
    );
}

#[tokio::test]
async fn rotations_disable_retire_and_emergency_preserve_original_response_binding() {
    let f = Fixture::new();
    f.create(owner()).await;
    f.change(1, "enable", BridgeChange::Enable).await.unwrap();
    let admitted = f
        .store
        .bridge(&f.id)
        .await
        .unwrap()
        .unwrap()
        .admission_pin()
        .unwrap();
    let mut rotated = config();
    rotated.connector.key_id = "key-2".into();
    rotated.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    f.change(
        2,
        "rotate",
        BridgeChange::Update {
            config: Box::new(rotated),
            previous_work: None,
        },
    )
    .await
    .unwrap();
    let record = f.store.bridge(&f.id).await.unwrap().unwrap();
    assert_eq!(
        record.pinned(&admitted).unwrap().config.connector.key_id,
        "key-1"
    );
    assert_eq!(record.current().unwrap().config.connector.key_id, "key-2");
    f.change(
        3,
        "disable",
        BridgeChange::Disable {
            accepted_work: AcceptedWorkPolicy::DrainOriginal,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        f.store
            .bridge(&f.id)
            .await
            .unwrap()
            .unwrap()
            .admission_pin(),
        Err(BridgeError::AdmissionStopped)
    );
    f.change(
        4,
        "retire",
        BridgeChange::Retire {
            accepted_work: AcceptedWorkPolicy::SuspendOriginal,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        f.change(5, "revive", BridgeChange::Enable).await,
        Err(BridgeError::Retired)
    );
    f.change(
        5,
        "stop-outbound",
        BridgeChange::Emergency {
            controls: BridgeEmergencyControls {
                outbound_stopped: true,
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
    f.change(
        6,
        "resume-outbound",
        BridgeChange::Emergency {
            controls: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        f.store
            .bridge(&f.id)
            .await
            .unwrap()
            .unwrap()
            .pinned(&admitted)
            .unwrap()
            .config
            .connector
            .key_id,
        "key-1"
    );
}

#[tokio::test]
async fn bot_identity_and_fixed_agent_rebinding_are_not_silent_updates() {
    let f = Fixture::new();
    f.create(owner()).await;
    let mut changed = config();
    changed.connector.bot_id = "different-bot".into();
    assert_eq!(
        f.change(
            1,
            "new-bot",
            BridgeChange::Update {
                config: Box::new(changed),
                previous_work: None
            }
        )
        .await,
        Err(BridgeError::ImmutableIdentity)
    );
    let mut changed = config();
    changed.placement = BridgePlacementPolicy::ExistingAgent {
        target: BridgeAgentTarget {
            sandbox_id: crate::SandboxId("sbx-1".into()),
            workspace_id: crate::WorkspaceId("wks-1".into()),
            worktree_id: crate::WorktreeId("wkt-1".into()),
            agent_id: crate::AgentId("agt-1".into()),
        },
    };
    assert_eq!(
        f.change(
            1,
            "rebind",
            BridgeChange::Update {
                config: Box::new(changed),
                previous_work: None
            }
        )
        .await,
        Err(BridgeError::RebindingPolicyRequired)
    );
}

#[tokio::test]
async fn mismatched_owner_route_callback_and_profile_facts_cannot_mint_a_proof() {
    let config = config();
    for reason in [
        BridgeSetupBlock::BindingMismatch,
        BridgeSetupBlock::CallbackConflict,
        BridgeSetupBlock::CustodyUnavailable,
        BridgeSetupBlock::RenewableAuthorityUnavailable,
        BridgeSetupBlock::ProfileUnavailable,
        BridgeSetupBlock::AuthorityExpired,
    ] {
        let mut facts = observation(&owner(), &config);
        match reason {
            BridgeSetupBlock::BindingMismatch => facts.key_owner = organization(),
            BridgeSetupBlock::CallbackConflict => facts.callback_conflict = true,
            BridgeSetupBlock::CustodyUnavailable => facts.custody_owner = organization(),
            BridgeSetupBlock::RenewableAuthorityUnavailable => {
                facts.renewable_sandbox_authority = false
            }
            BridgeSetupBlock::ProfileUnavailable => facts.exact_profiles_available = false,
            BridgeSetupBlock::AuthorityExpired => facts.deadline = tokio::time::Instant::now(),
            _ => unreachable!(),
        }
        assert_eq!(
            BridgeValidation::from_observation(&owner(), &config, facts).unwrap_err(),
            BridgeError::ConnectionRequired(reason)
        );
    }
    let mut facts = observation(&owner(), &config);
    facts.routes.push(BridgeRouteObservation {
        route_id: "unselected-route".into(),
        owner: owner(),
        bot_id: config.connector.bot_id.clone(),
        key_id: config.connector.key_id.clone(),
        enabled: true,
    });
    assert!(BridgeValidation::from_observation(&owner(), &config, facts).is_err());
    for (revision, key_version) in [(0, 3), (u64::MAX, 3), (1, 0), (1, -1)] {
        let mut facts = observation(&owner(), &config);
        facts.custody_revision = revision;
        facts.key_state_version = key_version;
        assert_eq!(
            BridgeValidation::from_observation(&owner(), &config, facts).unwrap_err(),
            BridgeError::ConnectionRequired(BridgeSetupBlock::CustodyUnavailable),
        );
    }
}

#[tokio::test(start_paused = true)]
async fn connection_proof_cannot_outlive_verification_or_authorize_another_config() {
    let config = config();
    let proof =
        BridgeValidation::from_observation(&owner(), &config, observation(&owner(), &config))
            .unwrap();
    assert_eq!(proof.custody_pin().reference(), &config.connector.custody);
    assert_eq!(proof.custody_pin().revision(), 1);
    assert_eq!(proof.custody_pin().key_state_version(), 3);
    let mut changed = config.clone();
    changed.connector.key_id = "different-key".into();
    assert!(proof.require(&owner(), &changed).is_err());
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(
        proof.require(&owner(), &config),
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::AuthorityExpired
        ))
    );
}

#[test]
fn configuration_rejects_unbounded_history_inputs_and_credential_urls() {
    let mut huge = config();
    huge.admission = BridgeAdmissionPolicy::Allowlist {
        sender_ids: (0..512).map(|i| format!("{i:0256}")).collect(),
    };
    assert_eq!(
        huge.validate(),
        Err(BridgeError::InvalidConfiguration("configuration_size"))
    );
    let mut credential = config();
    credential.connector.callback_url = "https://cma.example/bridge?token=fixture".into();
    assert_eq!(
        credential.validate(),
        Err(BridgeError::InvalidConfiguration("connector_url"))
    );
}

#[tokio::test(start_paused = true)]
async fn org_authority_cannot_be_renewed_by_slow_connection_validation() {
    let f = Fixture::new();
    f.create(organization()).await;
    f.organizations.lifetime.store(1, Ordering::SeqCst);
    f.validator.delay.store(2, Ordering::SeqCst);
    assert_eq!(
        f.change(1, "enable", BridgeChange::Enable).await,
        Err(BridgeError::AuthorityUnavailable)
    );
    assert_eq!(f.store.bridge(&f.id).await.unwrap().unwrap().revision, 1);
}

#[tokio::test]
async fn another_issuer_with_same_subject_cannot_read_manage_or_page_a_bridge() {
    let f = Fixture::new();
    f.create(owner()).await;
    let mut foreign = context("human");
    foreign.credential.issuer = "https://unrelated.example".into();
    assert_eq!(
        f.service.detail(&foreign, None, &f.id).await,
        Err(BridgeError::NotFound)
    );
    assert_eq!(
        f.service.validate(&foreign, None, &f.id).await,
        Err(BridgeError::NotFound)
    );
    let page = f
        .service
        .list(&foreign, None, &owner(), None, 1)
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert!(page.next_cursor.is_none());
    assert_eq!(
        f.service
            .mutate(
                &foreign,
                None,
                BridgeMutation {
                    id: f.id.clone(),
                    expected_revision: 1,
                    idempotency_key: "enable".into(),
                    change: BridgeChange::Enable
                }
            )
            .await,
        Err(BridgeError::NotFound)
    );
}

#[tokio::test(start_paused = true)]
async fn custody_management_lease_preserves_actor_and_original_deadline_and_revision() {
    let f = Fixture::new();
    f.create(organization()).await;
    f.organizations.lifetime.store(1, Ordering::SeqCst);
    let lease = f
        .service
        .management_lease(&context("human"), Some("fixture-auth"), &f.id, 1)
        .await
        .unwrap();
    assert_eq!(lease.record().owner, organization());
    assert_eq!(lease.actor(), &context("human").actor);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(
        f.service.validate_management(&lease).await,
        Err(BridgeError::AuthorityUnavailable)
    );
    f.organizations.lifetime.store(60, Ordering::SeqCst);
    let lease = f
        .service
        .management_lease(&context("human"), Some("fixture-auth"), &f.id, 1)
        .await
        .unwrap();
    f.change(
        1,
        "controls",
        BridgeChange::Emergency {
            controls: Default::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        f.service.validate_management(&lease).await,
        Err(BridgeError::Conflict)
    );
    *f.organizations.role.lock().unwrap() = Some(OrganizationRole::Viewer);
    assert!(matches!(
        f.service
            .management_lease(&context("human"), Some("fixture-auth"), &f.id, 2)
            .await,
        Err(BridgeError::Forbidden)
    ));
}
