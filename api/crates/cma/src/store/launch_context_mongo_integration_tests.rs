use std::sync::atomic::{AtomicU64, Ordering};

use layer2_core::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId,
    WorkspaceProfileId, WorktreeId,
};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, MAX_PAGE_SIZE};
use layer2_core::sandbox::{
    Agent, AgentOrigin, AgentStatus, AgentStopReason, AgentWorkState, AgentWorkStep,
    ConversationState, Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep, TypedFailure, Workspace,
    WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};
use layer2_core::trigger_domain::{LaunchContextReader, LaunchResolutionError, TriggerId};
use mongodb::Client;

use super::sandbox_mongo::MongoSandboxStore;
use super::sandboxes::sandbox_document;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const LATEST: &str = "2026-09-05T00:02:00Z";
static DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn context_store(prefix: &str) -> (Client, MongoSandboxStore, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
    let client = Client::with_uri_str(uri)
        .await
        .expect("test Mongo connects");
    let sequence = DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("cma_821_context_{prefix}_{}_{sequence}", std::process::id());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (client, store, name)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn context_matches_exact_lineage_and_selects_newest_deterministically() {
    let (client, store, name) = context_store("selection").await;
    let owner = SubjectId::new("visitor");
    let sandbox_profile = SandboxProfileId("sbp_target".into());

    let mut old_sandbox = sandbox("sbx_match_old", &owner, &sandbox_profile, 91, NOW);
    old_sandbox.status = SandboxStatus::Failed;
    old_sandbox.failure = Some(TypedFailure {
        class: "service_not_granted".into(),
        detail: "github".into(),
    });
    store.create_sandbox(old_sandbox.clone()).await.unwrap();
    let failed_sandbox = store
        .newest_matching_sandbox(&owner, &sandbox_profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed_sandbox.id, old_sandbox.id);
    let tied_low = sandbox("sbx_match_a", &owner, &sandbox_profile, 3, LATER);
    let mut tied_high = sandbox("sbx_match_z", &owner, &sandbox_profile, 7, LATER);
    tied_high.status = SandboxStatus::Retiring;
    let mut retired = sandbox("sbx_retired", &owner, &sandbox_profile, 8, LATEST);
    retired.status = SandboxStatus::Retired;
    retired.retired_at = Some(LATEST.into());
    let wrong_profile = sandbox(
        "sbx_wrong_profile",
        &owner,
        &SandboxProfileId("sbp_other".into()),
        1,
        LATEST,
    );
    let other_owner = sandbox(
        "sbx_other_owner",
        &SubjectId::new("other"),
        &sandbox_profile,
        1,
        LATEST,
    );
    for candidate in [
        tied_low,
        tied_high.clone(),
        retired,
        wrong_profile,
        other_owner,
    ] {
        SandboxStore::create_sandbox(&store, candidate)
            .await
            .unwrap();
    }

    let selected = store
        .newest_matching_sandbox(&owner, &sandbox_profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.id, tied_high.id);
    assert_eq!(selected.profile.revision, 7);

    let workspace_profile = WorkspaceProfileId("wkp_target".into());
    let mut old_workspace = workspace("wks_match_old", &tied_high, &workspace_profile, 88, NOW);
    old_workspace.status = WorkspaceStatus::Failed;
    old_workspace.failed = old_sandbox.failure.clone();
    store.create_workspace(old_workspace.clone()).await.unwrap();
    let failed_workspace = store
        .matching_workspace(&owner, &tied_high.id, &workspace_profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed_workspace.id, old_workspace.id);
    let tied_workspace_low = workspace("wks_match_a", &tied_high, &workspace_profile, 4, LATER);
    let mut tied_workspace_high =
        workspace("wks_match_z", &tied_high, &workspace_profile, 6, LATER);
    tied_workspace_high.status = WorkspaceStatus::Removing;
    let mut removed = workspace("wks_removed", &tied_high, &workspace_profile, 9, LATEST);
    removed.status = WorkspaceStatus::Removed;
    removed.removed_at = Some(LATEST.into());
    let wrong_workspace_profile = workspace(
        "wks_wrong_profile",
        &tied_high,
        &WorkspaceProfileId("wkp_other".into()),
        1,
        LATEST,
    );
    for candidate in [
        tied_workspace_low,
        tied_workspace_high.clone(),
        removed,
        wrong_workspace_profile,
    ] {
        WorkspaceStore::create_workspace(&store, candidate)
            .await
            .unwrap();
    }

    let selected = store
        .matching_workspace(&owner, &tied_high.id, &workspace_profile)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.id, tied_workspace_high.id);
    assert_eq!(selected.profile.revision, 6);

    let trigger = TriggerId("trg_target".into());
    let old_agent = agent(
        "agt_match_old",
        &owner,
        AgentStatus::Ready,
        trigger_origin(&trigger, "lch_old"),
        NOW,
    );
    let tied_agent_low = agent(
        "agt_match_a",
        &owner,
        AgentStatus::Ready,
        trigger_origin(&trigger, "lch_a"),
        LATER,
    );
    let mut tied_agent_high = agent(
        "agt_match_z",
        &owner,
        AgentStatus::Ready,
        trigger_origin(&trigger, "lch_z"),
        LATER,
    );
    tied_agent_high.sandbox_id = SandboxId("sbx_previous_launch".into());
    tied_agent_high.workspace_id = WorkspaceId("wks_previous_launch".into());
    tied_agent_high.profile.id = AgentProfileId("agp_previous_target".into());
    tied_agent_high.profile.revision = 41;
    let stopped = agent(
        "agt_stopped",
        &owner,
        AgentStatus::Stopped,
        trigger_origin(&trigger, "lch_stopped"),
        LATEST,
    );
    let other_trigger = agent(
        "agt_other_trigger",
        &owner,
        AgentStatus::Ready,
        AgentOrigin::Trigger {
            trigger_id: "trg_other".into(),
            launch_id: "lch_other".into(),
        },
        LATEST,
    );
    let manual = agent(
        "agt_manual",
        &owner,
        AgentStatus::Ready,
        AgentOrigin::Manual,
        LATEST,
    );
    for candidate in [
        old_agent,
        tied_agent_low,
        tied_agent_high.clone(),
        stopped,
        other_trigger,
        manual,
    ] {
        AgentStore::create_agent(&store, candidate).await.unwrap();
    }

    let selected = store
        .newest_live_trigger_agent(&owner, &trigger)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.id, tied_agent_high.id);
    assert_eq!(selected.sandbox_id.as_str(), "sbx_previous_launch");
    assert_eq!(selected.workspace_id.as_str(), "wks_previous_launch");
    assert_eq!(selected.profile.revision, 41);
    assert!(store
        .newest_live_trigger_agent(&owner, &TriggerId("trg_missing".into()))
        .await
        .unwrap()
        .is_none());

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn context_follows_bounded_pages() {
    let (client, store, name) = context_store("pages").await;
    let owner = SubjectId::new("visitor");
    let target = SandboxProfileId("sbp_target".into());
    let other = SandboxProfileId("sbp_other".into());
    let mut documents = Vec::new();
    for index in 0..=usize::from(MAX_PAGE_SIZE) {
        let profile = if index == usize::from(MAX_PAGE_SIZE) {
            &target
        } else {
            &other
        };
        let candidate = sandbox(&format!("sbx_page_{index:03}"), &owner, profile, 1, NOW);
        documents.push(sandbox_document(&candidate, 0).unwrap());
    }
    store.sandboxes.insert_many(documents).await.unwrap();

    let selected = store
        .newest_matching_sandbox(&owner, &target)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.id.as_str(), "sbx_page_200");

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn context_rejects_corrupt_scope_and_trigger_origin() {
    let (client, store, name) = context_store("corrupt").await;
    let owner = SubjectId::new("visitor");
    let profile = SandboxProfileId("sbp_target".into());
    let wrong_owner = SubjectId::new("other");
    let mut forged = sandbox("sbx_forged", &wrong_owner, &profile, 1, NOW);
    forged.name = "forged".into();
    let mut document = sandbox_document(&forged, 0).unwrap();
    document.insert("owner", owner.as_str());
    store.sandboxes.insert_one(document).await.unwrap();

    assert!(matches!(
        store.newest_matching_sandbox(&owner, &profile).await,
        Err(LaunchResolutionError::Corrupt(_))
    ));

    let parent = sandbox("sbx_parent", &owner, &profile, 1, NOW);
    let workspace_profile = WorkspaceProfileId("wkp_target".into());
    let mut foreign_child = workspace("wks_foreign", &parent, &workspace_profile, 1, NOW);
    foreign_child.owner = wrong_owner;
    WorkspaceStore::create_workspace(&store, foreign_child)
        .await
        .unwrap();
    assert!(matches!(
        store
            .matching_workspace(&owner, &parent.id, &workspace_profile)
            .await,
        Err(LaunchResolutionError::Corrupt(_))
    ));

    let lifecycle_parent = sandbox("sbx_lifecycle", &owner, &profile, 1, NOW);
    let mut bad_workspace = workspace(
        "wks_bad_lifecycle",
        &lifecycle_parent,
        &workspace_profile,
        1,
        NOW,
    );
    bad_workspace.removed_at = Some(NOW.into());
    WorkspaceStore::create_workspace(&store, bad_workspace)
        .await
        .unwrap();
    assert!(matches!(
        store
            .matching_workspace(&owner, &lifecycle_parent.id, &workspace_profile)
            .await,
        Err(LaunchResolutionError::Corrupt(_))
    ));

    let lifecycle_owner = SubjectId::new("lifecycle-owner");
    let lifecycle_trigger = TriggerId("trg_lifecycle".into());
    let mut bad_agent = agent(
        "agt_bad_lifecycle",
        &lifecycle_owner,
        AgentStatus::Ready,
        trigger_origin(&lifecycle_trigger, "lch_lifecycle"),
        NOW,
    );
    bad_agent.deleted_at = Some(NOW.into());
    AgentStore::create_agent(&store, bad_agent).await.unwrap();
    assert!(matches!(
        store
            .newest_live_trigger_agent(&lifecycle_owner, &lifecycle_trigger)
            .await,
        Err(LaunchResolutionError::Corrupt(_))
    ));

    let trigger = TriggerId("trg_target".into());
    let invalid_origin = agent(
        "agt_invalid_origin",
        &owner,
        AgentStatus::Ready,
        trigger_origin(&trigger, ""),
        NOW,
    );
    AgentStore::create_agent(&store, invalid_origin)
        .await
        .unwrap();
    assert!(matches!(
        store.newest_live_trigger_agent(&owner, &trigger).await,
        Err(LaunchResolutionError::Corrupt(_))
    ));

    client.database(&name).drop().await.unwrap();
}

fn sandbox(
    id: &str,
    owner: &SubjectId,
    profile_id: &SandboxProfileId,
    profile_revision: u32,
    created_at: &str,
) -> Sandbox {
    Sandbox {
        id: SandboxId(id.into()),
        owner: owner.clone(),
        name: id.replacen("sbx_", "", 1),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: profile_id.clone(),
            revision: profile_revision,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, created_at.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: created_at.into(),
        created_at: created_at.into(),
        updated_at: created_at.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn workspace(
    id: &str,
    sandbox: &Sandbox,
    profile_id: &WorkspaceProfileId,
    profile_revision: u32,
    created_at: &str,
) -> Workspace {
    let name = id.replacen("wks_", "", 1);
    Workspace {
        id: WorkspaceId(id.into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: profile_id.clone(),
            revision: profile_revision,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory { name: name.clone() },
                name: name.clone(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: name.clone(),
        path: format!("/home/cma/{name}"),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, created_at.into()),
        next_worktree_number: 1,
        created_at: created_at.into(),
        ready_at: Some(created_at.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

fn agent(
    id: &str,
    owner: &SubjectId,
    status: AgentStatus,
    origin: AgentOrigin,
    created_at: &str,
) -> Agent {
    let stopped = status == AgentStatus::Stopped;
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        workspace_id: WorkspaceId(format!("wks_{id}")),
        sandbox_id: SandboxId(format!("sbx_{id}")),
        owner: owner.clone(),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_initial".into()),
            revision: 1,
            digest: "e".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some(format!("conversation-{id}")),
            runtime_generation: Some(1),
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, created_at.into()),
        failure: None,
        title: Some(format!("Agent {id}")),
        created_at: created_at.into(),
        updated_at: created_at.into(),
        ready_at: Some(created_at.into()),
        stopped_at: stopped.then(|| created_at.into()),
        requested_stop_reason: None,
        stop_reason: stopped.then_some(AgentStopReason::Owner),
        deleted_at: None,
        origin,
        revision: 1,
    }
}

fn trigger_origin(trigger_id: &TriggerId, launch_id: &str) -> AgentOrigin {
    AgentOrigin::Trigger {
        trigger_id: trigger_id.as_str().into(),
        launch_id: launch_id.into(),
    }
}
