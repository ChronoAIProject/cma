use super::*;
use layer3_adapter::{AgentHomeRequest, WorktreeRequest};

pub(in crate::store) async fn hierarchy(
    store: &MongoSandboxStore,
) -> (Agent, Sandbox, Workspace, Worktree) {
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(store).await;
    let request = admission(store, &plan, "owner", "agent-pair").await;
    let SandboxOrigin::Lifecycle { operation_id } = request.sandbox.origin.clone() else {
        unreachable!()
    };
    let SandboxAdmissionResult::Created(mut parent) = admit_at(store, request, NOW).await.unwrap()
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
    parent.credential.reaches = record.payload.plan.reaches.clone();
    parent.work.execution = Some(record.payload.plan.execution);
    parent.work.step = SandboxWorkStep::WaitForCredentialRotation;
    parent.revision += 1;
    store.save_sandbox(previous, parent.clone()).await.unwrap();
    let mut input = crate::store::agent_access_fixtures::admission(&workspace, "prepared");
    input.profile.content = plan.warm.workspaces[0].agents[0].content.clone();
    input.profile.digest = content_digest(&input.profile.content).unwrap();
    input.origin = AgentOrigin::Lifecycle { operation_id };
    input.creation_key = format!("lifecycle:{}", content_digest(&input.origin).unwrap());
    let admitted = store.admit_agent(input).await.unwrap();
    let agent = store
        .claim_agent_work(
            &admitted.creation().agent.id,
            "pair-worker",
            NOW.into(),
            EXPIRED.into(),
        )
        .await
        .unwrap()
        .unwrap();
    let tree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = store.get_workspace(&workspace.id).await.unwrap().unwrap();
    let parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    (agent, parent, workspace, tree)
}

pub(in crate::store) fn intent(
    agent: &Agent,
    parent: &Sandbox,
    workspace: &Workspace,
    tree: &Worktree,
) -> AgentWorkMutation {
    let worktree = WorktreeRequest::new(
        "worktree:workspace:wt-1:add:v1",
        &workspace.name,
        &tree.name,
        tree.base_commit.clone(),
    );
    let home = AgentHomeRequest::new(
        format!("agent:{}:home:create:v1", agent.id),
        crate::sandbox::runtime_options(agent, parent, tree),
        String::new(),
        vec![],
    );
    let mut next = agent.clone();
    next.work.cleanup_debt = vec![
        AgentCleanupDebt {
            step: AgentWorkStep::AddWorktree,
            operation_id: worktree.operation_id,
            request_sha256: worktree.request_sha256,
        },
        AgentCleanupDebt {
            step: AgentWorkStep::MaterializeHome,
            operation_id: home.operation_id,
            request_sha256: home.request_sha256,
        },
    ];
    next.revision += 1;
    AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: "pair-worker".into(),
        fence: agent.work.lease.as_ref().unwrap().fence,
        agent: next,
        worktree: None,
        appended_at: NOW.into(),
        sandbox_event: None,
        agent_logs: vec![],
    }
}

pub(in crate::store) fn completion(agent: &Agent) -> AgentWorkMutation {
    let mut next = agent.clone();
    let fence = agent.work.lease.as_ref().unwrap().fence;
    next.work.receipts = next
        .work
        .cleanup_debt
        .drain(..)
        .map(|debt| AgentWorkReceipt {
            step: debt.step,
            operation_id: debt.operation_id,
            request_sha256: debt.request_sha256,
            completed_at: NOW.into(),
        })
        .collect();
    next.work
        .advance(
            "pair-worker",
            fence,
            AgentWorkStep::OpenConversation,
            NOW.into(),
        )
        .unwrap();
    next.revision += 1;
    next.conversation.last_seq += 2;
    AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: "pair-worker".into(),
        fence,
        agent: next,
        worktree: None,
        appended_at: NOW.into(),
        sandbox_event: None,
        agent_logs: vec![
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::AddWorktree,
            },
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::MaterializeHome,
            },
        ],
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_agent_pair_publishes_exact_receipts_and_transactional_parent_fences() {
    let (client, name, store) = database().await;
    let (agent, parent, workspace, tree) = hierarchy(&store).await;
    assert!(store
        .prepared_agent_replay_matches(&agent, &parent, &workspace, &tree)
        .await
        .unwrap());
    let pending = intent(&agent, &parent, &workspace, &tree);
    store.commit_agent_work(pending.clone()).await.unwrap();
    let done = completion(&pending.agent);
    store.commit_agent_work(done.clone()).await.unwrap();
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap(),
        done.agent
    );
    for (collection, id) in [
        (&store.sandboxes, parent.id.as_str()),
        (&store.workspaces, workspace.id.as_str()),
        (&store.worktrees, tree.id.as_str()),
        (&store.native_prepared, parent.id.as_str()),
    ] {
        let row = collection.find_one(doc! {"_id":id}).await.unwrap().unwrap();
        assert_eq!(row.get_i64("agent_prepared_replay_fence").unwrap(), 2);
    }
    assert!(matches!(
        store.commit_agent_work(done).await,
        Err(SandboxStoreError::Conflict)
    ));
    let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::WorkStepCompleted { .. }))
            .count(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_agent_pair_refuses_changed_boot_owner_profile_home_and_debt() {
    let (client, name, store) = database().await;
    let (agent, parent, workspace, tree) = hierarchy(&store).await;
    for kind in 0..7 {
        let (mut a, mut p, mut w, mut t) = (
            agent.clone(),
            parent.clone(),
            workspace.clone(),
            tree.clone(),
        );
        match kind {
            0 => p.runtime.as_mut().unwrap().boot_id = Some("c".repeat(32)),
            1 => p.lifecycle.home.as_mut().unwrap().request.volume_id = "foreign".into(),
            2 => a.owner = SubjectId::new("foreign"),
            3 => w.profile.id = WorkspaceProfileId("wsp_foreign".into()),
            4 => t.base_commit = "d".repeat(40),
            5 => a.work.warm_conversation_id = Some("foreign".into()),
            6 => {
                a.work.cleanup_debt = vec![AgentCleanupDebt {
                    step: AgentWorkStep::AddWorktree,
                    operation_id: "foreign".into(),
                    request_sha256: "e".repeat(64),
                }]
            }
            _ => unreachable!(),
        }
        assert!(
            !store
                .prepared_agent_replay_matches(&a, &p, &w, &t)
                .await
                .unwrap(),
            "case {kind}"
        );
    }
    let mut bad = intent(&agent, &parent, &workspace, &tree);
    bad.agent.work.cleanup_debt[0].request_sha256 = "e".repeat(64);
    assert!(matches!(
        store.commit_agent_work(bad).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_agent_pair_publication_refuses_parent_pause_after_intent() {
    let (client, name, store) = database().await;
    let (agent, mut parent, workspace, tree) = hierarchy(&store).await;
    let pending = intent(&agent, &parent, &workspace, &tree);
    store.commit_agent_work(pending.clone()).await.unwrap();
    let revision = parent.revision;
    parent.begin_pause(NOW.into(), None).unwrap();
    store.save_sandbox(revision, parent).await.unwrap();
    assert!(matches!(
        store.commit_agent_work(completion(&pending.agent)).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap(),
        pending.agent
    );
    client.database(&name).drop().await.unwrap();
}
