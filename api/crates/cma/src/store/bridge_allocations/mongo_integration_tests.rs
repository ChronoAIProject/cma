//! Disposable fixtures only. No provider resources or real users are mutated.
use super::*;

use crate::store::MongoBridgeAllocationStore;
use layer2_core::bridge::{allocation::*, ingress::*, partition::*};
use layer2_core::profiles::{
    content_digest, AgentProfileContent, AgentProfileSnapshot, ProfileCapacityBounds,
    SandboxProfileContent, SandboxProfileSnapshot, WorkspaceProfileContent,
    WorkspaceProfileSnapshot,
};

#[path = "sandbox_admission/mongo_tests.rs"]
mod sandbox_admission;

#[path = "channel_provisioning/tests.rs"]
mod channel_provisioning;
pub(crate) use channel_provisioning::compatibility;

async fn setup() -> (
    Client,
    String,
    Arc<MongoBridgeStore>,
    BridgeService,
    BridgeId,
    MongoBridgeAllocationStore,
) {
    setup_with_callback_route(false).await
}

async fn setup_with_callback_route(
    callback_route: bool,
) -> (
    Client,
    String,
    Arc<MongoBridgeStore>,
    BridgeService,
    BridgeId,
    MongoBridgeAllocationStore,
) {
    let (client, database, bridges) = fixture().await;
    let svc = service(bridges.clone());
    let id = BridgeService::creation_id(&caller(), "create").unwrap();
    let mut configured = config();
    configured.partition = BridgePartitionPolicy::ConversationSender;
    if callback_route {
        configured.connector.callback_url = format!(
            "https://cma.example/api/v1/bridge-callbacks/{}",
            id.as_str()
        );
    }
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(configured),
            },
        ),
    )
    .await
    .unwrap();
    admit_custody(&svc, &bridges, &id, 1).await;
    svc.mutate(
        &caller(),
        None,
        change(&id, 1, "enable", BridgeChange::Enable),
    )
    .await
    .unwrap();
    let store = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
        .await
        .unwrap();
    (client, database, bridges, svc, id, store)
}

async fn proposal(
    svc: &BridgeService,
    bridges: &MongoBridgeStore,
    id: &BridgeId,
    sender: &str,
    previous: Option<&AllocationRecord>,
) -> (
    AllocationAdmission,
    AllocationAuthority,
    BridgeManagementLease,
) {
    proposal_with_owner_limit(svc, bridges, id, sender, previous, 3).await
}

async fn proposal_with_owner_limit(
    svc: &BridgeService,
    bridges: &MongoBridgeStore,
    id: &BridgeId,
    sender: &str,
    previous: Option<&AllocationRecord>,
    owner_live_sandbox_limit: u16,
) -> (
    AllocationAdmission,
    AllocationAuthority,
    BridgeManagementLease,
) {
    let bridge = bridges.bridge(id).await.unwrap().unwrap();
    let config = &bridge.current().unwrap().config;
    let inbox = StoredInbox {
        id: "a".repeat(64),
        owner: owner(),
        pin: bridge.admission_pin().unwrap(),
        bot_id: config.connector.bot_id.clone(),
        key_id: config.connector.key_id.clone(),
        message_id: "message".into(),
        correlation_id: "correlation".into(),
        envelope: InboxEnvelope {
            platform: "telegram".into(),
            route_id: "route-1".into(),
            chat_id: "-100123".into(),
            conversation_type: "group".into(),
            sender_id: sender.into(),
            sender_display_name: None,
            content_type: "text".into(),
            text: Some("fixture".into()),
            attachments: vec![],
            reply_to_message_id: None,
            reply_to_platform_message_id: None,
            protected_thread_context_present: false,
            timestamp: "2026-09-15T06:00:00Z".into(),
        },
        attestation: CallbackAttestation {
            issuer: config.connector.issuer.clone(),
            signing_key_id: "signing".into(),
            token_id: "token".into(),
            issued_at: 100,
            expires_at: 200,
            body_sha256: "b".repeat(64),
        },
        relay: RelayCorroboration::AbsentInboxOnly,
        admission: InboxAdmission::Accepted,
        capabilities: None,
        execution_hold: None,
        accepted_at: 101,
        expires_at: 3601,
        reply_expires_at: None,
    };
    let mut observation = PlatformCapabilityObservation::unknown(&inbox).unwrap();
    observation.sender_kind = ChannelSenderKind::Human;
    observation.event_kind = ChannelEventKind::UserMessage;
    observation.loop_evidence = LoopEvidence::NotLoop;
    let source = NormalizedPartitionSource::from_observation(&inbox, &bridge, observation).unwrap();
    let mut admission = AllocationAdmission::new(bridge.clone(), &source).unwrap();
    if let Some(previous) = previous {
        admission = admission.after_reset(previous).unwrap();
    }
    let (authority, management) =
        authority_for_admission(svc, bridge, &admission, owner_live_sandbox_limit).await;
    (admission, authority, management)
}

/// Profile preparation accepts the already checked admission; it does not
/// manufacture callback identity or pre-create the allocation's target rows.
async fn authority_for_admission(
    svc: &BridgeService,
    bridge: BridgeRecord,
    admission: &AllocationAdmission,
    owner_live_sandbox_limit: u16,
) -> (AllocationAuthority, BridgeManagementLease) {
    let management = svc
        .management_lease(&caller(), None, &bridge.id, bridge.revision)
        .await
        .unwrap();
    let config = &bridge.current().unwrap().config;
    let sandbox_content: SandboxProfileContent = serde_json::from_value(serde_json::json!({
        "cpu_millis":1000,"memory_mib":1024,"agent_provider":"codex",
        "model_access":"chronoai_managed","max_workspaces":1,"max_agents_per_workspace":1
    }))
    .unwrap();
    let workspace_content: WorkspaceProfileContent = serde_json::from_value(serde_json::json!({
        "location":{"type":"directory","name":"fixture"},"name":"fixture"
    }))
    .unwrap();
    let agent_content: AgentProfileContent = serde_json::from_value(serde_json::json!({
        "thinking_effort":"low","permission_level":"guarded"
    }))
    .unwrap();
    let sandbox = SandboxProfileSnapshot {
        id: SandboxProfileId("sbp-1".into()),
        revision: 1,
        digest: content_digest(&sandbox_content).unwrap(),
        content: sandbox_content,
    };
    let workspace = WorkspaceProfileSnapshot {
        id: WorkspaceProfileId("wkp-1".into()),
        revision: 1,
        digest: content_digest(&workspace_content).unwrap(),
        content: workspace_content,
    };
    let agent = AgentProfileSnapshot {
        id: AgentProfileId("agp-1".into()),
        revision: 1,
        digest: content_digest(&agent_content).unwrap(),
        content: agent_content,
    };
    let digest = AllocationProfiles::digest(&sandbox, &workspace, &agent).unwrap();
    let plan = AllocationProfilePlan::prepare(
        &management,
        sandbox,
        workspace,
        agent,
        &digest,
        AllocationCapabilityObservation {
            owner_live_sandbox_limit,
            baseline_services: Vec::new(),
            owner: owner(),
            profiles_digest: digest.clone(),
            capacity_bounds: ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8000,
                min_memory_mib: 128,
                max_memory_mib: 32768,
            },
            one_sandbox_workspace_worktree_agent_available: true,
            runtime_model_services_skills_available: true,
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
        },
    )
    .unwrap();
    let connection = Authority
        .validate_connection(&owner(), config)
        .await
        .unwrap();
    let authority = AllocationAuthority::Provisioning(Box::new(
        AllocationProvisioningProof::new(bridge, admission.identity(), plan, connection).unwrap(),
    ));
    (authority, management)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_concurrent_first_messages_and_restart_retain_one_tree() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let (admission, authority, _) = proposal(&svc, &bridges, &id, "sender", None).await;
    let a = AllocationReservation::new(admission.clone(), authority.clone(), now()).unwrap();
    let b = AllocationReservation::new(admission.clone(), authority.clone(), now()).unwrap();
    let (a, b) = tokio::join!(allocations.reserve(a), allocations.reserve(b));
    let first = a.unwrap();
    let second = b.unwrap();
    assert!(first == second);
    let reopened = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
        .await
        .unwrap();
    assert!(
        reopened
            .current(admission.identity())
            .await
            .unwrap()
            .unwrap()
            == first
    );
    let a = AllocationMutation::claim(first.clone(), authority.clone(), "worker-a", now(), 60_000)
        .unwrap();
    let b = AllocationMutation::claim(first.clone(), authority, "worker-b", now(), 60_000).unwrap();
    let (a, b) = tokio::join!(allocations.mutate(a), reopened.mutate(b));
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(AllocationError::Conflict)) || matches!(b, Err(AllocationError::Conflict))
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_allocations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_reset_preserves_predecessor_and_capacity_counts_retained_generations() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let (admission, authority, _) = proposal(&svc, &bridges, &id, "sender", None).await;
    let first = allocations
        .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
        .await
        .unwrap();
    let (admission, authority, management) =
        proposal(&svc, &bridges, &id, "sender", Some(&first)).await;
    let next = allocations
        .reserve(
            AllocationReservation::reset(
                admission.clone(),
                authority,
                first.clone(),
                management,
                now(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(next.identity.generation, 2);
    assert_eq!(next.predecessor.as_ref().unwrap().id, first.id);
    assert_ne!(next.placement.target(), first.placement.target());
    let previous = allocations
        .generation(&first.identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(previous.state, AllocationState::Retired);
    assert_eq!(previous.placement, first.placement);
    assert!(
        allocations
            .current(admission.identity())
            .await
            .unwrap()
            .unwrap()
            == next
    );
    let (admission, authority, management) =
        proposal(&svc, &bridges, &id, "sender", Some(&next)).await;
    let exhausted =
        AllocationReservation::reset(admission, authority, next.clone(), management, now())
            .unwrap();
    assert!(matches!(
        allocations.reserve(exhausted).await,
        Err(AllocationError::Capacity)
    ));
    assert!(allocations.current(&next.identity).await.unwrap().unwrap() == next);
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_allocations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_rechecks_custody_before_new_and_replayed_reservation() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let (admission, authority, management) = proposal(&svc, &bridges, &id, "sender", None).await;
    allocations
        .reserve(AllocationReservation::new(admission.clone(), authority.clone(), now()).unwrap())
        .await
        .unwrap();
    let replay = AllocationReservation::new(admission, authority, now()).unwrap();
    let (other, other_authority, _) = proposal(&svc, &bridges, &id, "other-sender", None).await;
    let new = AllocationReservation::new(other, other_authority, now()).unwrap();
    bridges
        .credentials
        .revoke(&management, &config().connector.custody, 1, &"f".repeat(64))
        .await
        .unwrap();
    assert!(matches!(
        allocations.reserve(replay).await,
        Err(AllocationError::AuthorityUnavailable)
    ));
    assert!(matches!(
        allocations.reserve(new).await,
        Err(AllocationError::AuthorityUnavailable)
    ));
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_allocations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_lost_receipt_takeover_reconciles_without_second_start_or_reset() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let (admission, authority, _) = proposal(&svc, &bridges, &id, "sender", None).await;
    let reserved = allocations
        .reserve(AllocationReservation::new(admission, authority.clone(), now()).unwrap())
        .await
        .unwrap();
    let claim = allocations
        .mutate(
            AllocationMutation::claim(reserved, authority.clone(), "worker-a", now(), 200).unwrap(),
        )
        .await
        .unwrap();
    let lease = claim.lease.clone().unwrap();
    let started = allocations
        .mutate(
            AllocationMutation::progress(
                claim,
                authority.clone(),
                &lease,
                AllocationProgress::Start,
                now(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(210)).await;
    let reopened = MongoBridgeAllocationStore::open(bridges.clone(), None, &database)
        .await
        .unwrap();
    let latest = reopened
        .generation(&started.identity)
        .await
        .unwrap()
        .unwrap();
    let taken = reopened
        .mutate(
            AllocationMutation::claim(latest, authority.clone(), "worker-b", now(), 60_000)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(taken.operation_id, started.operation_id);
    assert_eq!(taken.placement, started.placement);
    assert!(AllocationMutation::progress(
        taken.clone(),
        authority.clone(),
        taken.lease.as_ref().unwrap(),
        AllocationProgress::Start,
        now()
    )
    .is_err());
    let (admission, next_authority, management) =
        proposal(&svc, &bridges, &id, "sender", Some(&taken)).await;
    assert!(matches!(
        AllocationReservation::reset(admission, next_authority, taken.clone(), management, now()),
        Err(AllocationError::ResetRequired)
    ));
    let observed = AllocationResourceObservation {
        owner: owner(),
        operation_id: taken.operation_id.clone(),
        target: taken.placement.target().clone(),
        receipt_digest: "d".repeat(64),
        profiles_digest: taken.profiles_digest().map(str::to_owned),
    };
    let confirmed = reopened
        .mutate(
            AllocationMutation::progress(
                taken.clone(),
                authority,
                taken.lease.as_ref().unwrap(),
                AllocationProgress::Confirm(observed),
                now(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed.state, AllocationState::Ready);
    assert_eq!(confirmed.placement, started.placement);
    client.database(&database).drop().await.unwrap();
}

async fn renew_retained(
    svc: &BridgeService,
    bridges: &MongoBridgeStore,
    record: &AllocationRecord,
) -> Result<AllocationAuthority, AllocationError> {
    renew_retained_with_owner_limit(svc, bridges, record, 3).await
}

async fn renew_retained_with_owner_limit(
    svc: &BridgeService,
    bridges: &MongoBridgeStore,
    record: &AllocationRecord,
    owner_live_sandbox_limit: u16,
) -> Result<AllocationAuthority, AllocationError> {
    let bridge = bridges
        .bridge(&record.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    let management = svc
        .management_lease(&caller(), None, &bridge.id, bridge.revision)
        .await
        .unwrap();
    let AllocationPlacement::NewSandbox { profiles, .. } = &record.placement else {
        unreachable!()
    };
    let plan = AllocationProfilePlan::prepare_for_generation(
        &management,
        record.pin.clone(),
        profiles.sandbox.clone(),
        profiles.workspace.clone(),
        profiles.agent.clone(),
        &profiles.trust_digest,
        AllocationCapabilityObservation {
            owner_live_sandbox_limit,
            baseline_services: Vec::new(),
            owner: owner(),
            profiles_digest: profiles.trust_digest.clone(),
            capacity_bounds: ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8000,
                min_memory_mib: 128,
                max_memory_mib: 32768,
            },
            one_sandbox_workspace_worktree_agent_available: true,
            runtime_model_services_skills_available: true,
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
        },
    )?;
    let connection = Authority
        .validate_connection(&owner(), &record.config)
        .await
        .unwrap();
    AllocationProvisioningProof::new(bridge, &record.identity, plan, connection)
        .map(|proof| AllocationAuthority::Provisioning(Box::new(proof)))
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_original_generation_drains_after_rotation_and_suspend_blocks_old_proof() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let (admission, authority, _) = proposal(&svc, &bridges, &id, "sender", None).await;
    let retained = allocations
        .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
        .await
        .unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            2,
            "disable-drain",
            BridgeChange::Disable {
                accepted_work: AcceptedWorkPolicy::DrainOriginal,
            },
        ),
    )
    .await
    .unwrap();
    let mut rotated = retained.config.clone();
    rotated.connector.key_id = "key-two".into();
    rotated.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            3,
            "rotate",
            BridgeChange::Update {
                config: Box::new(rotated),
                previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
            },
        ),
    )
    .await
    .unwrap();
    let renewed = renew_retained(&svc, &bridges, &retained).await.unwrap();
    assert_eq!(renewed.config().unwrap().connector.key_id, "key-1");
    let leased = allocations
        .mutate(
            AllocationMutation::claim(retained.clone(), renewed.clone(), "worker", now(), 60_000)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(leased.pin, retained.pin);
    assert_eq!(leased.placement, retained.placement);
    let pending = AllocationMutation::progress(
        leased.clone(),
        renewed,
        leased.lease.as_ref().unwrap(),
        AllocationProgress::Start,
        now(),
    )
    .unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            4,
            "retire-suspend",
            BridgeChange::Retire {
                accepted_work: AcceptedWorkPolicy::SuspendOriginal,
            },
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        allocations.mutate(pending).await,
        Err(AllocationError::Conflict)
    ));
    assert!(matches!(
        renew_retained(&svc, &bridges, &retained).await,
        Err(AllocationError::AuthorityUnavailable)
    ));
    let unchanged = allocations
        .generation(&retained.identity)
        .await
        .unwrap()
        .unwrap();
    assert!(!unchanged.effect_started);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn allocation_distinct_partition_capacity_race_and_corrupt_indexes_fail_closed() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let (admission, authority, _) = proposal(&svc, &bridges, &id, "first", None).await;
    let first = allocations
        .reserve(AllocationReservation::new(admission, authority, now()).unwrap())
        .await
        .unwrap();
    let (a, aa, _) = proposal(&svc, &bridges, &id, "second", None).await;
    let (b, ba, _) = proposal(&svc, &bridges, &id, "third", None).await;
    let (a, b) = tokio::join!(
        allocations.reserve(AllocationReservation::new(a, aa, now()).unwrap()),
        allocations.reserve(AllocationReservation::new(b, ba, now()).unwrap()),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(AllocationError::Capacity)) || matches!(b, Err(AllocationError::Capacity))
    );
    client
        .database(&database)
        .collection::<Document>("bridge_allocations")
        .update_one(
            doc! {"_id":&first.id},
            doc! {"$set":{"key_hash":"0".repeat(64)}},
        )
        .await
        .unwrap();
    assert!(matches!(
        allocations.generation(&first.identity).await,
        Err(AllocationError::Corrupt)
    ));
    client.database(&database).drop().await.unwrap();
}
