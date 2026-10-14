use super::*;
use futures_util::TryStreamExt;
use layer2_core::profiles::*;
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{ServiceId, SubjectId, WorkspaceProfileId};
use layer3_adapter::agent_sandbox::*;
use layer3_adapter::{HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, RuntimeImage};

mod claim_race_tests;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:01:00Z";
const EXPIRED: &str = "2026-09-05T00:30:01Z";

fn plan() -> NativePreparedPlan {
    let sandbox = crate::store::agent_access_fixtures::sandbox("prepared")
        .profile
        .content;
    NativePreparedPlan::new(
        sandbox,
        WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "workspace".into(),
            },
            name: "workspace".into(),
            setup_commands: vec![],
            agent_profiles: vec![],
        },
        crate::agent_lifecycle::validation::agent_content_for(
            &Default::default(),
            crate::store::LifecycleBehavior::PromptRepository,
        )
        .unwrap(),
        vec![],
        format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
        Default::default(),
        HomeVolumePolicy {
            mode: HomeVolumeMode::NativeVctFullHomeV2,
            storage_class: "standard-rwo".into(),
            capacity_gib: 8,
            template_name: String::new(),
            template_generation: String::new(),
        },
    )
    .unwrap()
}

fn actual_profiles(
    plan: &NativePreparedPlan,
) -> (SandboxProfileSnapshot, WorkspaceProfileSnapshot) {
    let mut workspace = plan.warm.workspaces[0].profile.clone();
    workspace.id = WorkspaceProfileId("wsp_private_actual".into());
    workspace.content.agent_profiles.clear();
    workspace.digest = content_digest(&workspace.content).unwrap();
    let mut sandbox = plan.warm.profile.clone();
    sandbox.id = layer2_core::SandboxProfileId("sbp_private_actual".into());
    sandbox.content.workspace_profiles = vec![WorkspaceProfileRef::new(
        workspace.id.clone(),
        workspace.revision,
    )];
    sandbox.digest = content_digest(&sandbox.content).unwrap();
    (sandbox, workspace)
}

fn ready(mut record: Record) -> Record {
    let warm = &mut record.payload;
    let mut home = SandboxHome::reserve(&warm.id, record.plan.home_policy.clone()).unwrap();
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                image: RuntimeImage {
                    uri: record.plan.warm.runtime_version.clone(),
                    labels: std::collections::BTreeMap::from([
                        (layer3_adapter::RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: record.plan.home_policy.clone(),
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    warm.home = Some(Box::new(home));
    warm.runtime = Some(RuntimeBinding {
        runtime_id: format!("runtime-{}", warm.id),
        runtime_version: record.plan.warm.runtime_version.clone(),
        provision_generation: Some(1),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    warm.state = WarmSandboxState::Ready;
    warm.installed_request_sha256 = Some("a".repeat(64));
    warm.workspaces[0].resolved_commit = Some("a".repeat(40));
    warm.workspaces[0].setup_ready = true;
    warm.workspaces[0].agents[0].skills = Some(vec![]);
    warm.workspaces[0].agents[0].conversation_id = Some("prepared-session".into());
    record.validate().unwrap();
    record
}

#[test]
fn native_prepared_normalizes_only_private_references() {
    let plan = plan();
    let (sandbox, workspace) = actual_profiles(&plan);
    let agent = plan.warm.workspaces[0].agents[0].content.clone();
    assert!(plan.matches(&sandbox, &workspace, &agent, &[]));
    for case in 0..8 {
        let (mut s, mut w, mut a) = (sandbox.clone(), workspace.clone(), agent.clone());
        let mut reaches = vec![];
        match case {
            0 => s.content.cpu_millis += 1,
            1 => s.content.runtime_image = Some(plan.warm.runtime_version.clone()),
            2 => w.content.setup_commands.push("echo surprise".into()),
            3 => {
                w.content.agent_profiles = plan.warm.workspaces[0]
                    .profile
                    .content
                    .agent_profiles
                    .clone()
            }
            4 => a.model = Some("different".into()),
            5 => a.permission_level = PermissionLevel::Guarded,
            6 => s.content.workspace_profiles[0].revision += 1,
            _ => reaches.push(ServiceId::new("extra")),
        }
        s.digest = content_digest(&s.content).unwrap();
        w.digest = content_digest(&w.content).unwrap();
        assert!(!plan.matches(&s, &w, &a, &reaches), "case {case}");
    }
}

#[test]
fn native_prepared_record_rejects_unpinned_runtime_home_and_query_fields() {
    let p = plan();
    let record = ready(Record {
        payload: WarmSandbox::new(p.warm.clone(), NOW.into()).unwrap(),
        plan: p,
        ownership: None,
    });
    for case in 0..5 {
        let mut changed = record.clone();
        match case {
            0 => changed.payload.home = None,
            1 => changed.payload.runtime.as_mut().unwrap().boot_id = None,
            2 => {
                changed
                    .payload
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .request
                    .pool
                    .spec
                    .cpu_millis += 1
            }
            3 => changed.payload.state = WarmSandboxState::Claimed,
            _ => changed.payload.plan.execution.clone_depth += 1,
        }
        assert!(changed.validate().is_err(), "case {case}");
    }
    let mut value = document(&record).unwrap();
    value.insert("state", "claimed");
    assert!(read(&value).is_err());
}

async fn database() -> (mongodb::Client, String, MongoSandboxStore) {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("requires local disposable replica set");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_native_prepared_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    (client, name, store)
}
async fn records(store: &MongoSandboxStore) -> Vec<Record> {
    store
        .native_prepared
        .find(doc! {"kind":"reservation"})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
        .iter()
        .map(|r| read(r).unwrap())
        .collect()
}
async fn publish_ready(store: &MongoSandboxStore) -> Record {
    let old = records(store)
        .await
        .into_iter()
        .find(|r| r.payload.state == WarmSandboxState::Preparing)
        .unwrap();
    let record = ready(old);
    let mut value = document(&record).unwrap();
    value.insert("lease_until", NOW);
    value.insert("lease_token", mongodb::bson::Bson::Null);
    value.insert("check_after", LATER);
    store
        .native_prepared
        .replace_one(doc! {"_id":record.payload.id.as_str()}, value)
        .await
        .unwrap();
    record
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_concurrent_target_fence_caps_and_expiry_preserves_leases() {
    let (client, name, store) = database().await;
    let plan = plan();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (s, p) = (store.clone(), plan.clone());
        tasks.spawn(async move {
            s.reconcile_native_prepared_target(Some(&p), 2, NOW)
                .await
                .unwrap()
        });
    }
    while let Some(task) = tasks.join_next().await {
        task.unwrap();
    }
    assert_eq!(records(&store).await.len(), 2);
    let claim = store
        .claim_native_prepared_work(NOW, LATER)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        store
            .claim_native_prepared_work(NOW, LATER)
            .await
            .unwrap()
            .unwrap()
            .record
            .id,
        claim.record.id
    );
    assert!(store
        .claim_native_prepared_work(NOW, LATER)
        .await
        .unwrap()
        .is_none());
    store
        .reconcile_native_prepared_target(None, 0, NOW)
        .await
        .unwrap();
    assert!(store
        .finish_native_prepared_work(&claim, claim.record.clone(), NOW, NOW)
        .await
        .is_err());
    assert!(
        store
            .claim_native_prepared_work(NOW, LATER)
            .await
            .unwrap()
            .is_none(),
        "retirement waits original lease"
    );
    store
        .reconcile_native_prepared_target(Some(&plan), 2, EXPIRED)
        .await
        .unwrap();
    assert_eq!(
        records(&store).await.len(),
        2,
        "retirement occupies bounded capacity"
    );
    let cleanup = store
        .claim_native_prepared_work(EXPIRED, "2026-09-05T00:31:00Z")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleanup.record.state, WarmSandboxState::Retiring);
    let mut retired = cleanup.record.clone();
    retired.state = WarmSandboxState::Retired;
    store
        .finish_native_prepared_work(&cleanup, retired, EXPIRED, EXPIRED)
        .await
        .unwrap();
    store
        .reconcile_native_prepared_target(Some(&plan), 2, EXPIRED)
        .await
        .unwrap();
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.payload.state.counts_toward_target())
            .count(),
        1
    );
    assert!(store
        .reconcile_native_prepared_target(Some(&plan), 3, NOW)
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}

async fn admission(
    store: &MongoSandboxStore,
    plan: &NativePreparedPlan,
    owner: &str,
    key: &str,
) -> SandboxAdmission {
    let database = store.client.database(&store.sandboxes.namespace().db);
    let ops = crate::store::MongoAgentLifecycleStore::open(&database)
        .await
        .unwrap();
    let result = ops
        .create(
            &SubjectId::new(owner),
            key,
            crate::store::LifecycleInput {
                behavior: crate::store::LifecycleBehavior::PromptRepository,
                request: serde_json::from_value(serde_json::json!({"name":"Prepared test"}))
                    .unwrap(),
                resolved_image: None,
                capacity: Some(layer3_adapter::SandboxCapacity {
                    cpu_millis: 1000,
                    memory_mib: 2048,
                }),
                capabilities: vec![],
                actor: None,
            },
        )
        .await
        .unwrap();
    let crate::store::CreateResult::Created(operation_id) = result else {
        panic!("new operation")
    };
    let (profile, workspace) = actual_profiles(plan);
    let revision = WorkspaceProfileRevision {
        profile_id: workspace.id.clone(),
        revision: workspace.revision,
        content: workspace.content,
        digest: workspace.digest,
        published_at: NOW.into(),
    };
    database
        .collection::<Document>(crate::store::sandbox_mongo::WORKSPACE_PROFILE_REVISIONS)
        .replace_one(
            doc! {"_id":format!("{}:{:010}",revision.profile_id,revision.revision)},
            crate::store::profile_documents::revision_document(
                revision.profile_id.as_str(),
                revision.revision,
                &revision,
            )
            .unwrap(),
        )
        .upsert(true)
        .await
        .unwrap();
    let mut parent =
        crate::store::sandbox_admission_mongo_integration_tests::sandbox("sbx_cold", key);
    parent.name = "prepared".into();
    parent.owner = SubjectId::new(owner);
    parent.origin = SandboxOrigin::Lifecycle { operation_id };
    parent.profile = profile;
    parent.credential.reaches = vec![];
    parent.credential.sign_in = SandboxSignInState::NotRequired;
    parent.limits = SandboxLimits {
        max_workspaces: 4,
        max_agents_per_workspace: 4,
    };
    SandboxAdmission::new(
        parent,
        SandboxAdmissionName::Allocate("prepared".into()),
        20,
        NOW.into(),
    )
    .unwrap()
}

async fn admit_at(
    store: &MongoSandboxStore,
    admission: SandboxAdmission,
    observed_at: &str,
) -> Result<SandboxAdmissionResult, SandboxStoreError> {
    store
        .commit_sandbox_admission_observing(admission, || observed_at.to_owned())
        .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_atomic_lifecycle_claim_is_exclusive_replayable_and_preserves_authority() {
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let request = record
        .payload
        .home
        .as_ref()
        .unwrap()
        .native_pool
        .as_ref()
        .unwrap()
        .request
        .clone();
    assert!(exact_claim_authority(&store, &request).await);
    let mut foreign = request.clone();
    foreign.pool.uid = "replacement".into();
    assert!(!exact_claim_authority(&store, &foreign).await);
    let a = admission(&store, &plan, "owner-a", "a").await;
    let b = admission(&store, &plan, "owner-b", "b").await;
    let (ra, rb) = tokio::join!(
        admit_at(&store, a.clone(), NOW),
        admit_at(&store, b.clone(), NOW)
    );
    let (created_a, created_b) = match (ra.unwrap(), rb.unwrap()) {
        (SandboxAdmissionResult::Created(a), SandboxAdmissionResult::Created(b)) => (a, b),
        _ => panic!("both fresh"),
    };
    assert_ne!(created_a.id, created_b.id);
    let (warm, request) = if created_a.runtime.is_some() {
        (created_a, a)
    } else {
        (created_b, b)
    };
    assert_eq!(warm.id, record.payload.id);
    assert_eq!(warm.owner, request.sandbox.owner);
    assert_eq!(warm.profile, request.sandbox.profile);
    assert!(!warm.credential.delivered);
    assert!(warm.credential.current.is_none());
    assert!(
        !exact_claim_authority(
            &store,
            &record
                .payload
                .home
                .as_ref()
                .unwrap()
                .native_pool
                .as_ref()
                .unwrap()
                .request
        )
        .await
    );

    assert_eq!(
        store.get_sandbox(&warm.id).await.unwrap().unwrap(),
        warm,
        "fresh owned authority follows Claimed read"
    );
    let SandboxAdmissionResult::Existing(replay) = admit_at(&store, request, NOW).await.unwrap()
    else {
        panic!("same operation replays")
    };
    assert_eq!(replay, warm);
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.ownership.is_some())
            .count(),
        1
    );
    assert!(store
        .consumed_native_prepared_for_sandbox(&warm.id)
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_adoption_waits_for_parent_and_tool_marker_requires_exact_receipt() {
    use base64::Engine as _;
    use layer3_adapter::hosted::HostedAccessToolSource;
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let request = admission(&store, &plan, "owner", "adoption").await;
    let SandboxOrigin::Lifecycle { operation_id } = request.sandbox.origin.clone() else {
        unreachable!()
    };
    let SandboxAdmissionResult::Created(mut parent) = admit_at(&store, request, NOW).await.unwrap()
    else {
        panic!("fresh")
    };
    let (_, profile) = actual_profiles(&plan);
    let mut candidate = crate::store::agent_access_fixtures::workspace(&parent, "prepared");
    candidate.profile = profile;
    candidate.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    candidate.name = "workspace".into();
    candidate.path = "/home/cma/workspace".into();
    candidate.status = WorkspaceStatus::Preparing;
    candidate.resolved_commit = None;
    candidate.ready_at = None;
    candidate.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let WorkspaceAdmissionResult::Created(mut workspace) = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: candidate,
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: NOW.into(),
        })
        .await
        .unwrap()
    else {
        panic!("fresh workspace")
    };
    assert_eq!(workspace.id, record.payload.workspaces[0].id);
    let mut input = crate::store::agent_access_fixtures::admission(&workspace, "prepared");
    input.profile.content = plan.warm.workspaces[0].agents[0].content.clone();
    input.profile.digest = content_digest(&input.profile.content).unwrap();
    input.origin = AgentOrigin::Lifecycle { operation_id };
    input.creation_key = format!("lifecycle:{}", content_digest(&input.origin).unwrap());
    assert!(matches!(
        store.admit_agent(input.clone()).await,
        Err(SandboxStoreError::SandboxNotReady)
    ));
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = record.payload.workspaces[0].resolved_commit.clone();
    workspace.ready_at = Some(NOW.into());
    workspace.work.step = WorkspaceWorkStep::None;
    workspace.revision += 1;
    store
        .save_workspace(workspace.revision - 1, workspace.clone())
        .await
        .unwrap();
    parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let previous = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.credential = crate::store::agent_access_fixtures::sandbox("prepared").credential;
    parent.work.step = SandboxWorkStep::WaitForCredentialRotation;
    parent.revision += 1;
    store.save_sandbox(previous, parent.clone()).await.unwrap();
    let admitted = store.admit_agent(input.clone()).await.unwrap();
    let mut agent = admitted.creation().agent.clone();
    assert_eq!(agent.id, record.payload.workspaces[0].agents[0].id);
    assert_eq!(
        agent.work.warm_conversation_id.as_deref(),
        Some("prepared-session")
    );
    assert_ne!(agent.work.step, AgentWorkStep::WaitForWorkspace);
    assert_eq!(
        store.admit_agent(input).await.unwrap().creation().agent.id,
        agent.id
    );
    agent.work.step = AgentWorkStep::OpenConversation;
    agent.work.next_fence = 3;
    agent.work.lease = Some(AgentWorkLease {
        holder: "prepared-worker".into(),
        fence: 2,
        expires_at: layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        ),
    });
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let snapshot = store.agent_provenance_snapshot(&agent.id).await.unwrap();
    let open = layer3_adapter::AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(
            &agent,
            &parent,
            snapshot.resource.lineage.worktree.as_ref().unwrap(),
        ),
        None,
    );
    let runtime =
        layer3_adapter::RuntimeHandle(parent.runtime.as_ref().unwrap().runtime_id.clone());
    let encryption = crate::store::AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([9; 32]),
    )
    .unwrap();
    let store = std::sync::Arc::new(store);
    let source = crate::store::agent_access_tools::MongoAgentAccessToolSource::new(
        store.clone(),
        "https://control.example.test",
        &encryption,
    )
    .unwrap();
    let binding = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    assert!(source
        .confirm_open(&binding, "wrong-conversation")
        .await
        .is_err());
    source
        .confirm_open(&binding, "prepared-session")
        .await
        .unwrap();
    source
        .confirm_open(&binding, "prepared-session")
        .await
        .unwrap();
    // Independent recovery recomputes the ordinary retained receipt after successful handoff.
    let reopened = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    assert_ne!(binding, reopened);
    source
        .confirm_open(&reopened, "prepared-session")
        .await
        .unwrap();
    let mut changed = snapshot.clone();
    changed
        .resource
        .lineage
        .agent
        .as_mut()
        .unwrap()
        .work
        .warm_conversation_id = Some("foreign".into());
    let mut session = client.start_session().await.unwrap();
    assert!(
        super::prepared_conversation(&store, &changed, &open, &mut session)
            .await
            .is_err()
    );
    changed = snapshot.clone();
    changed
        .resource
        .lineage
        .sandbox
        .runtime
        .as_mut()
        .unwrap()
        .boot_id = Some("c".repeat(32));
    assert!(
        super::prepared_conversation(&store, &changed, &open, &mut session)
            .await
            .unwrap()
            .is_none()
    );
    client.database(&name).drop().await.unwrap();
}

async fn exact_claim_authority(
    store: &MongoSandboxStore,
    request: &NativePoolClaimRequest,
) -> bool {
    store
        .unowned_home_reservation(&SandboxId(request.home.sandbox_id.clone()))
        .await
        .unwrap()
        .is_some_and(|home| {
            !home.released
                && home
                    .native_pool
                    .as_ref()
                    .is_some_and(|native| !native.cleaned && native.request == *request)
        })
}
