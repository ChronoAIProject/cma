use super::*;
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, WorktreeRequest};

pub(crate) async fn hierarchy(store: &MongoSandboxStore) -> (Agent, Sandbox, Workspace, Worktree) {
    hierarchy_with_plan(store, plan()).await
}

pub(crate) async fn adoption_policy(
    client: &mongodb::Client,
    name: &str,
    agent_id: &str,
    revision: i64,
    enabled: bool,
) {
    adoption_policies(client, name, agent_id, agent_id, revision, enabled).await;
}

pub(crate) async fn adoption_policies(
    client: &mongodb::Client,
    name: &str,
    writer_agent: &str,
    recovery_agent: &str,
    revision: i64,
    enabled: bool,
) {
    use crate::store::agent_log_segments::{
        schema,
        writer_gate::{self, GateName, Policy},
    };
    let db = client.database(name);
    if revision == 0 {
        schema::migrate(&db).await.unwrap();
        crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    }
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    for (gate, agent_id) in [
        (GateName::Segments, writer_agent),
        (GateName::CapacityRecovery, recovery_agent),
    ] {
        let policy = Policy {
            enabled,
            agents: enabled.then(|| agent_id.to_owned()).into_iter().collect(),
            ..Policy::default()
        };
        writer_gate::replace(&db, gate, revision, policy, &mut session)
            .await
            .unwrap();
    }
    session.commit_transaction().await.unwrap();
}

pub(crate) async fn hierarchy_with_plan(
    store: &MongoSandboxStore,
    plan: NativePreparedPlan,
) -> (Agent, Sandbox, Workspace, Worktree) {
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(store).await;
    let request = admission(store, &plan, "owner", "agent-open-ready").await;
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
    let previous = parent.revision;
    parent.status = SandboxStatus::Installing;
    parent.revision += 1;
    store.save_sandbox(previous, parent.clone()).await.unwrap();
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
    parent.lifecycle.home = record.payload.home.clone();
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
            "open-worker",
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
    let mut current = agent.clone();
    let options = crate::sandbox::runtime_options(&current, &parent, &tree);
    let add = WorktreeRequest::new(
        "worktree:workspace:wt-1:add:v1",
        "workspace",
        "wt-1",
        tree.base_commit.clone(),
    );
    let home = AgentHomeRequest::new(
        format!("agent:{}:home:create:v1", current.id),
        options.clone(),
        current.profile.content.system_prompt.clone(),
        vec![],
    );
    let open = AgentOpenRequest::new(format!("agent:{}:open:v1", current.id), options, None);
    current.work.receipts = [
        (
            AgentWorkStep::AddWorktree,
            add.operation_id,
            add.request_sha256,
        ),
        (
            AgentWorkStep::MaterializeHome,
            home.operation_id,
            home.request_sha256,
        ),
    ]
    .into_iter()
    .map(|(step, operation_id, request_sha256)| AgentWorkReceipt {
        step,
        operation_id,
        request_sha256,
        completed_at: NOW.into(),
    })
    .collect();
    current.work.step = AgentWorkStep::OpenConversation;
    current.work.cleanup_debt = vec![AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: open.operation_id,
        request_sha256: open.request_sha256,
    }];
    current.revision += 1;
    store
        .save_agent(agent.revision, current.clone())
        .await
        .unwrap();
    (current, parent, workspace, tree)
}

pub(in crate::store) fn completion(agent: &Agent, tree: &Worktree) -> AgentWorkMutation {
    let stamp = NOW.to_string();
    let fence = agent.work.lease.as_ref().unwrap().fence;
    let mut next = agent.clone();
    let debt = next.work.cleanup_debt.pop().unwrap();
    next.work.receipts.push(AgentWorkReceipt {
        step: debt.step,
        operation_id: debt.operation_id,
        request_sha256: debt.request_sha256,
        completed_at: stamp.clone(),
    });
    next.conversation.conversation_id = next.work.warm_conversation_id.clone();
    next.conversation.runtime_generation = Some(1);
    next.work
        .advance(
            "open-worker",
            fence,
            AgentWorkStep::PublishConnection,
            stamp.clone(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, stamp.clone(), None)
        .unwrap();
    next.conversation.last_seq += 3;
    let mut ready = tree.clone();
    ready.transition(WorktreeStatus::Ready).unwrap();
    let logs = vec![
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::OpenConversation,
        },
        AgentLogKind::ConversationOpened {
            conversation_id: next.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: "open-worker".into(),
        fence,
        agent: next,
        worktree: Some(VersionedWorktree {
            expected_revision: tree.revision,
            worktree: ready,
        }),
        appended_at: stamp,
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: agent.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }),
        agent_logs: logs,
    }
}
