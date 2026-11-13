//! A local replica set proves recovery does not duplicate or destroy retained work.
use super::*;
use layer2_core::sandbox::store::{
    AgentControlStore, AgentControlStoreError, AgentProvenanceStore,
};
use layer2_core::sandbox::{
    Agent, AgentCleanupDebt, AgentStatus, AgentWorkStep, FirstMessageRecord, TypedFailure,
    WorktreeStatus,
};
use layer2_core::{Authorization, Caller, Capability};
use layer3_adapter::WorktreeRequest;

fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
async fn fail_created(
    store: &MongoSandboxStore,
    request: AgentAdmission,
    workspace: &Workspace,
) -> Agent {
    let created = store.admit_agent(request).await.unwrap();
    fail_retained(store, created.creation().agent.clone(), workspace).await
}

async fn fail_retained(
    store: &MongoSandboxStore,
    mut agent: Agent,
    workspace: &Workspace,
) -> Agent {
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let request = WorktreeRequest::new(
        format!("worktree:{}:{}:add:v1", workspace.name, worktree.name),
        workspace.name.clone(),
        worktree.name.clone(),
        worktree.base_commit.clone(),
    );
    agent.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::AddWorktree,
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
    });
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    let revision = agent.revision;
    agent
        .fail(
            TypedFailure {
                class: "agent_runtime_rejected".into(),
                detail: "runtime_mutation_outcome_unknown: worktree add deadline elapsed".into(),
            },
            NOW.into(),
        )
        .unwrap();
    store.save_agent(revision, agent.clone()).await.unwrap();
    let revision = worktree.revision;
    worktree.transition(WorktreeStatus::Failed).unwrap();
    store.save_worktree(revision, worktree).await.unwrap();
    agent
}
async fn failed_fixture(
    suffix: &str,
) -> (Client, String, MongoSandboxStore, Sandbox, Workspace, Agent) {
    let (client, database, store, parent, workspace, mut request) = fixture(suffix).await;
    request.first_message = Some(
        FirstMessageRecord::new(
            request.creation_key.clone(),
            "Preserve my original request",
            NOW,
        )
        .unwrap(),
    );
    let agent = fail_created(&store, request, &workspace).await;
    // Ordinary admission already advances the parent activity revision.
    let parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    (client, database, store, parent, workspace, agent)
}
async fn recover(
    store: &MongoSandboxStore,
    agent: &Agent,
    key: &str,
) -> Result<Agent, AgentControlStoreError> {
    store
        .begin_agent_worktree_recovery(
            &agent.id,
            &owner(),
            key,
            &WorktreeId::generate(),
            NOW.into(),
        )
        .await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worktree_recovery_concurrent_replay_retains_history_provenance_and_failed_path() {
    let (client, database, store, parent, workspace, original) =
        failed_fixture("recovery_replay").await;
    let old_worktree = store
        .get_worktree(&original.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let provenance = store
        .agent_provenance_snapshot(&original.id)
        .await
        .unwrap()
        .creation
        .unwrap();
    let log = store
        .agent_log
        .find_one(doc! {"agent_id":original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let foreign = Caller::new(
        SubjectId::new("foreign"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    assert!(matches!(
        store
            .begin_agent_worktree_recovery(
                &original.id,
                &foreign,
                "first",
                &WorktreeId::generate(),
                NOW.into()
            )
            .await,
        Err(AgentControlStoreError::NotFound)
    ));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = store.clone();
        let agent = original.clone();
        tasks.spawn(async move { recover(&store, &agent, "first").await });
    }
    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        results.push(result.unwrap().unwrap());
    }
    let next = results[0].clone();
    assert!(results.iter().all(|value| value == &next));
    assert_eq!(next.id, original.id);
    assert_eq!(next.profile, original.profile);
    assert_eq!(next.conversation, original.conversation);
    assert_eq!(
        next.work.worktree_recoveries[0].original_worktree,
        old_worktree
    );
    assert_eq!(
        next.work.worktree_recoveries[0].original_failure,
        original.failure.clone().unwrap()
    );
    let retained = store
        .get_worktree(&original.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.path, old_worktree.path);
    assert_eq!(retained.branch, old_worktree.branch);
    assert_eq!(retained.base_commit, old_worktree.base_commit);
    assert_eq!(retained.status, WorktreeStatus::Failed);
    assert_eq!(retained.agent_id, None);
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        store
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .next_worktree_number,
        3
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().unwrap(),
        parent
    );
    assert_eq!(
        store
            .agent_log
            .find_one(doc! {"agent_id":original.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        log
    );
    assert_eq!(
        store
            .agent_provenance_snapshot(&next.id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        provenance
    );
    let restarted = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(recover(&restarted, &original, "first").await.unwrap(), next);
    assert!(matches!(
        recover(&restarted, &original, "second").await,
        Err(AgentControlStoreError::WorktreeRecoveryConflict)
    ));
    let mut forged = next.clone();
    forged.work.worktree_recoveries[0].original_failure.detail =
        "runtime_mutation_outcome_unknown: rewritten".into();
    forged.revision += 1;
    assert_eq!(
        store.save_agent(next.revision, forged).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut deleted = next.clone();
    deleted.status = AgentStatus::Deleted;
    deleted.work.schedule(AgentWorkStep::None, NOW.into());
    deleted.deleted_at = Some(NOW.into());
    deleted.revision += 1;
    store
        .save_agent(next.revision, deleted.clone())
        .await
        .unwrap();
    store
        .workspaces
        .update_one(
            doc! {"_id":workspace.id.as_str()},
            doc! {"$set":{"owned_workspace_cleanup_admission_v1":"admitted"}},
        )
        .await
        .unwrap();
    assert_eq!(
        recover(&restarted, &original, "first").await.unwrap(),
        deleted
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worktree_recovery_competing_keys_allocate_one_successor() {
    let (client, database, store, _, _, original) = failed_fixture("recovery_keys").await;
    let (one, two) = tokio::join!(
        recover(&store, &original, "one"),
        recover(&store, &original, "two")
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(matches!(
        one.err().or_else(|| two.err()),
        Some(AgentControlStoreError::WorktreeRecoveryConflict)
    ));
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worktree_recovery_shares_quota_fence_with_ordinary_admission() {
    let (client, database, store, _, workspace, original) = failed_fixture("recovery_quota").await;
    let mut live = Vec::new();
    for index in 0..4 {
        live.push(
            store
                .admit_agent(admission(&workspace, &format!("quota-{index}")))
                .await
                .unwrap()
                .creation()
                .agent
                .clone(),
        );
    }
    assert!(matches!(
        recover(&store, &original, "recover").await,
        Err(AgentControlStoreError::LimitReached { limit: 4, live: 4 })
    ));
    let mut stopped = live[0].clone();
    stopped.status = AgentStatus::Stopped;
    stopped.revision += 1;
    store.save_agent(live[0].revision, stopped).await.unwrap();
    let (recovery, creation) = tokio::join!(
        recover(&store, &original, "recover"),
        store.admit_agent(admission(&workspace, "quota-racer"))
    );
    assert_eq!(
        usize::from(recovery.is_ok()) + usize::from(creation.is_ok()),
        1
    );
    assert_eq!(
        store
            .agents
            .count_documents(doc! {"status":{"$in":crate::store::agents::LIVE_STATUSES}})
            .await
            .unwrap(),
        4
    );
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 6);
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worktree_recovery_cleanup_fences_and_duplicate_successor_roll_back_atomically() {
    let (client, database, store, parent, workspace, original) =
        failed_fixture("recovery_fences").await;
    let original_workspace = store.get_workspace(&workspace.id).await.unwrap().unwrap();
    for (collection, id, marker) in [
        (
            &store.sandboxes,
            parent.id.as_str(),
            "channel_reuse_cleanup_execution",
        ),
        (
            &store.sandboxes,
            parent.id.as_str(),
            "owned_workspace_cleanup_admission_v1",
        ),
        (
            &store.workspaces,
            workspace.id.as_str(),
            "owned_workspace_cleanup_admission_v1",
        ),
        (
            &store.worktrees,
            original.worktree_id.as_str(),
            "channel_reuse_cleanup_execution",
        ),
        (&store.agents, original.id.as_str(), "channel_history_seal"),
    ] {
        collection
            .update_one(doc! {"_id":id}, doc! {"$set":{marker:"admitted"}})
            .await
            .unwrap();
        assert!(matches!(
            recover(&store, &original, "recover").await,
            Err(AgentControlStoreError::Conflict)
        ));
        assert_eq!(
            store.get_agent(&original.id).await.unwrap().unwrap(),
            original
        );
        assert_eq!(
            store.get_workspace(&workspace.id).await.unwrap().unwrap(),
            original_workspace
        );
        assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 1);
        collection
            .update_one(doc! {"_id":id}, doc! {"$unset":{marker:""}})
            .await
            .unwrap();
    }
    let mut collision = store
        .get_worktree(&original.worktree_id)
        .await
        .unwrap()
        .unwrap();
    collision.id = WorktreeId::generate();
    collision.name = "wt-99".into();
    collision.revision = 1;
    store.create_worktree(collision.clone()).await.unwrap();
    assert!(matches!(
        store
            .begin_agent_worktree_recovery(
                &original.id,
                &owner(),
                "recover",
                &collision.id,
                NOW.into()
            )
            .await,
        Err(AgentControlStoreError::WorktreeRecoveryConflict)
    ));
    assert_eq!(
        store.get_agent(&original.id).await.unwrap().unwrap(),
        original
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap().unwrap(),
        original_workspace
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn worktree_recovery_unknown_commit_uses_one_snapshot_and_never_allocates_twice() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = format!("recovery_unknown_{}", uuid::Uuid::new_v4().simple());
    let app_name = format!("cma-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::<(String, Document)>::new()));
    let observed = commands.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let database = format!("cma_{suffix}");
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let parent = sandbox(&suffix);
    store.create_sandbox(parent.clone()).await.unwrap();
    let workspace = workspace(&parent, &suffix);
    store.create_workspace(workspace.clone()).await.unwrap();
    let original = fail_created(&store, admission(&workspace, &suffix), &workspace).await;
    commands.lock().unwrap().clear();
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{
        "failCommands":["commitTransaction"],"appName":&app_name,"writeConcernError":{
            "code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected uncertain recovery commit"}}}).await.unwrap();
    let outcome = recover(&store, &original, "recover").await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let next = outcome.unwrap();
    let trace = commands.lock().unwrap().clone();
    let commits: Vec<_> = trace
        .iter()
        .filter(|(name, _)| name == "commitTransaction")
        .collect();
    assert_eq!(commits.len(), 5);
    assert!(commits
        .iter()
        .all(|(_, command)| command.get("txnNumber") == commits[0].1.get("txnNumber")));
    let last_commit = trace
        .iter()
        .rposition(|(name, _)| name == "commitTransaction")
        .unwrap();
    let reads: Vec<_> = trace[last_commit + 1..]
        .iter()
        .filter(|(name, _)| name == "find")
        .collect();
    assert_eq!(reads.len(), 5);
    let concern = reads[0].1.get_document("readConcern").unwrap();
    assert_eq!(concern.get_str("level").unwrap(), "snapshot");
    assert!(concern.get_timestamp("atClusterTime").is_ok());
    assert!(reads
        .iter()
        .all(|(_, command)| command.get_document("readConcern").unwrap() == concern));
    let restarted = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(
        recover(&restarted, &original, "recover").await.unwrap(),
        next
    );
    assert_eq!(store.worktrees.count_documents(doc! {}).await.unwrap(), 2);
    client.database(&database).drop().await.unwrap();
}

#[path = "agent_worktree_recovery_integrity_tests.rs"]
mod integrity;
