//! Real source/gate fixture; native transport is supplied separately by each test.
use super::*;
use crate::store::agent_log_segments::{schema, writer_gate};
use layer2_core::sandbox::{RuntimeBinding, SandboxHome};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

pub(super) async fn capacity_fixture(store: &MongoSandboxStore) -> Agent {
    let agent = ready_with_created(store, "activation").await;
    prepare_capacity(store, agent).await
}

pub(super) async fn prepare_capacity(store: &MongoSandboxStore, mut agent: Agent) -> Agent {
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    if sandbox.lifecycle.home.is_none() {
        sandbox.runtime = Some(RuntimeBinding {
            runtime_id: "retained-runtime".into(),
            runtime_version: "fixture".into(),
            boot_id: Some("a".repeat(32)),
            provision_generation: Some(1),
            bound_at: NOW.into(),
        });
        sandbox.lifecycle.runtime_generation_high_water = 1;
        let mut home = SandboxHome::reserve(
            &sandbox.id,
            HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: "empty".into(),
                template_generation: "seed".into(),
            },
        )
        .unwrap();
        home.bind(HomeVolumeReceipt {
            native_authority: None,
            volume_id: home.request.volume_id.clone(),
            request_sha256: home.request.computed_sha256(),
            storage_reference: "retained-home".into(),
            storage_generation: "original".into(),
        })
        .unwrap();
        sandbox.lifecycle.home = Some(Box::new(home));
        sandbox.revision += 1;
        store
            .sandboxes
            .update_one(
                doc! {"_id": sandbox.id.as_str()},
                doc! {"$set":
                crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap()},
            )
            .await
            .unwrap();
    }
    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
    worktree.base_commit = workspace.resolved_commit.clone().unwrap();
    worktree.revision += 1;
    replace_worktree_fixture(store, &worktree).await;
    agent.status = AgentStatus::Failed;
    agent.work.step = AgentWorkStep::None;
    agent.work.lease = None;
    agent.failure = Some(TypedFailure {
        class: "agent_log_capacity_exceeded".into(),
        detail: "fixture".into(),
    });
    agent.revision += 1;
    replace_agent_fixture(store, &agent).await;
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::PublishConnection,
                failure: agent.failure.clone().unwrap(),
            },
        )
        .await
        .unwrap();
    agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    sandbox.validate().expect("valid recovery parent fixture");
    agent
        .validate_prerequisites(&worktree)
        .expect("valid recovery Agent prerequisites");
    worktree
        .validate_prerequisites(&workspace)
        .expect("valid recovery Worktree prerequisites");
    workspace
        .validate_prerequisites(&sandbox)
        .expect("valid recovery Workspace prerequisites");
    let context = layer2_core::sandbox::agent_capacity_recovery::CapacityRecoveryContext {
        agent: &agent,
        sandbox: &sandbox,
        workspace: &workspace,
        worktree: &worktree,
    };
    assert_eq!(
        context.classify(&layer2_core::now_rfc3339()),
        layer2_core::sandbox::agent_capacity_recovery::CapacityRecoveryDisposition::EvidenceRequired,
        "fixture must reach evidence admission"
    );
    let db = store.client.database(&store.agents.namespace().db);
    schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    for name in [
        writer_gate::GateName::Segments,
        writer_gate::GateName::CapacityRecovery,
    ] {
        let (revision, mut policy) = match writer_gate::read(&db, name, &mut session).await.unwrap()
        {
            Some(current) => (current.revision, current.policy),
            None => (0, writer_gate::Policy::default()),
        };
        policy.enabled = true;
        policy.agents.push(agent.id.to_string());
        writer_gate::replace(&db, name, revision, policy, &mut session)
            .await
            .unwrap();
    }
    session.commit_transaction().await.unwrap();
    agent
}

// Adoption must prove the original Created row. The generic worker fixture
// deliberately starts at ConversationOpened and cannot qualify this contract.
pub(in crate::store::sandbox_mongo_integration_tests) async fn ready_with_created(
    store: &MongoSandboxStore,
    suffix: &str,
) -> Agent {
    let (_, worktree, claimed) = claimed_agent_work_fixture(store, suffix).await;
    store
        .append_agent_log(
            &claimed.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: claimed.workspace_id.clone(),
                worktree_id: claimed.worktree_id.clone(),
            },
        )
        .await
        .unwrap();
    let current = store.get_agent(&claimed.id).await.unwrap().unwrap();
    ready_claimed(store, worktree, current, suffix).await
}

pub(super) async fn ready_claimed(
    store: &MongoSandboxStore,
    mut worktree: Worktree,
    current: Agent,
    suffix: &str,
) -> Agent {
    let conversation_id = format!("conversation-{suffix}");
    let mut claimed = ready_publication_claim(current, &conversation_id);
    // Simulated preceding work steps change state and must advance revision;
    // the Created checkpoint cannot remain bound to this changed aggregate.
    claimed.revision += 1;
    replace_agent_fixture(store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut ready = claimed.clone();
    ready
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    ready
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id,
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    ready.conversation.last_seq += logs.len() as u64;
    let expected_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: ready.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision,
                worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: ready.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: logs,
        })
        .await
        .unwrap();
    ready
}
