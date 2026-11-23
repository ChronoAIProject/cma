use super::*;
use crate::store::native_prepared::tests::open_ready_fixture::{completion, hierarchy};
use layer2_core::sandbox::store::AgentConnectionRecoveryCandidate;
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, AgentWorkerStore, PreparedAgentOpenContext, SandboxStore,
    WorkspaceStore,
};
use layer2_core::sandbox::AgentStatus;

mod snapshot;

async fn fixture() -> (
    mongodb::Client,
    String,
    MongoSandboxStore,
    Agent,
    PreparedAgentOpenContext,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_prepared_open_{}", uuid::Uuid::new_v4().simple());
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, sandbox, workspace, worktree) = hierarchy(&store).await;
    (
        client,
        name,
        store,
        agent,
        PreparedAgentOpenContext {
            sandbox,
            workspace,
            worktree,
        },
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_is_atomic_and_restart_keeps_fresh_connection_pending() {
    let (client, name, store, agent, observed) = fixture().await;
    let done = completion(&agent, &observed.worktree);
    assert!(matches!(
        store.commit_agent_work(done.clone()).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert!(store
        .commit_prepared_agent_open_ready(done.clone(), observed.clone())
        .await
        .unwrap());
    for (collection, id) in [
        (&store.sandboxes, observed.sandbox.id.as_str()),
        (&store.workspaces, observed.workspace.id.as_str()),
        (&store.worktrees, observed.worktree.id.as_str()),
        (&store.native_prepared, observed.sandbox.id.as_str()),
    ] {
        let value = collection.find_one(doc! {"_id":id}).await.unwrap().unwrap();
        assert_eq!(value.get_i64("agent_prepared_open_ready_fence").unwrap(), 1);
    }
    drop(store);
    let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
    let ready = restarted.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(ready, done.agent);
    assert_eq!(ready.work.step, AgentWorkStep::PublishConnection);
    let request = AgentConnectionRecoveryCandidate::from_agent(&ready)
        .unwrap()
        .restart_request(done.appended_at.clone());
    assert_eq!(
        restarted
            .schedule_agent_connection_recovery(request)
            .await
            .unwrap(),
        layer2_core::sandbox::store::AgentConnectionRecoveryOutcome::AlreadyScheduled
    );
    assert!(!restarted
        .commit_prepared_agent_open_ready(done.clone(), observed)
        .await
        .unwrap());
    let logs = restarted
        .agent_log(&agent.id, agent.conversation.last_seq, 10)
        .await
        .unwrap();
    assert_eq!(
        logs.iter().map(|log| log.kind.clone()).collect::<Vec<_>>(),
        done.agent_logs
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_refuses_changed_observation_receipt_ownership_and_parent() {
    let (client, name, store, agent, observed) = fixture().await;
    for case in 0..9 {
        let mut observed = observed.clone();
        let mut done = completion(&agent, &observed.worktree);
        match case {
            0 => observed.sandbox.runtime.as_mut().unwrap().boot_id = Some("d".repeat(32)),
            1 => {
                observed
                    .sandbox
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .request
                    .volume_id = "foreign".into()
            }
            2 => observed.workspace.revision += 1,
            3 => observed.worktree.revision += 1,
            4 => done.agent.work.receipts[2].request_sha256 = "d".repeat(64),
            5 => done.agent.conversation.conversation_id = Some("foreign".into()),
            6 => done.fence += 1,
            7 => done.agent.profile.revision += 1,
            _ => observed.sandbox.credential.delivered = false,
        }
        assert!(
            !matches!(
                store.commit_prepared_agent_open_ready(done, observed).await,
                Ok(true)
            ),
            "case {case}"
        );
    }
    for (collection, id) in [
        (&store.sandboxes, observed.sandbox.id.as_str()),
        (&store.workspaces, observed.workspace.id.as_str()),
        (&store.worktrees, observed.worktree.id.as_str()),
        (&store.native_prepared, observed.sandbox.id.as_str()),
    ] {
        collection
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"owned_workspace_cleanup_admission_v1":{}}},
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .commit_prepared_agent_open_ready(
                    completion(&agent, &observed.worktree),
                    observed.clone()
                )
                .await,
            Err(SandboxStoreError::Conflict)
        ));
        collection
            .update_one(
                doc! {"_id":id},
                doc! {"$unset":{"owned_workspace_cleanup_admission_v1":""}},
            )
            .await
            .unwrap();
    }
    let mut changed = observed.workspace.clone();
    changed.revision += 1;
    store
        .save_workspace(observed.workspace.revision, changed)
        .await
        .unwrap();
    assert!(matches!(
        store
            .commit_prepared_agent_open_ready(
                completion(&agent, &observed.worktree),
                observed.clone()
            )
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_parent_write_conflicts_after_snapshot_read() {
    let (client, name, store, agent, observed) = fixture().await;
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    scope
        .find_one(&store.sandboxes, doc! {"_id":observed.sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut parent = observed.sandbox.clone();
    parent
        .begin_pause("2026-09-05T00:00:00Z".into(), None)
        .unwrap();
    store
        .save_sandbox(observed.sandbox.revision, parent)
        .await
        .unwrap();
    assert!(
        matches!(apply_publication_observed(&store,&completion(&agent,&observed.worktree),Some(&observed),None,&mut publication_history::AttemptHistory::default(),PublicationGuards::default(),scope).await,Err(ApplyError::Driver(error)) if is_transient(&error))
    );
    session.abort_transaction().await.unwrap();
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_unknown_commit_requires_complete_atomic_evidence() {
    let (client, name, store, agent, observed) = fixture().await;
    let done = completion(&agent, &observed.worktree);
    let mut session = client.start_session().await.unwrap();
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let rolled_back = apply_publication_observed(
        &store,
        &done,
        Some(&observed),
        None,
        &mut publication_history::AttemptHistory::default(),
        PublicationGuards::default(),
        scope,
    )
    .await
    .unwrap_or_else(|_| panic!("valid prepared publication"));
    session.abort_transaction().await.unwrap();
    assert!(
        !super::super::agent_worker_reconcile::reconcile_publication(&store, &rolled_back)
            .await
            .unwrap()
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = apply_publication_observed(
        &store,
        &done,
        Some(&observed),
        None,
        &mut publication_history::AttemptHistory::default(),
        PublicationGuards::default(),
        scope,
    )
    .await
    .unwrap_or_else(|_| panic!("valid prepared publication"));
    session.commit_transaction().await.unwrap();
    let at = session.operation_time().unwrap();
    assert!(
        super::super::agent_worker_reconcile::reconcile_publication_after_unknown(
            &store, &receipt, at
        )
        .await
        .unwrap()
    );
    let mut wrong = receipt.clone();
    wrong.worktree.as_mut().unwrap().branch = "foreign".into();
    assert!(
        !super::super::agent_worker_reconcile::reconcile_publication_after_unknown(
            &store, &wrong, at
        )
        .await
        .unwrap()
    );
    let mut wrong = receipt.clone();
    wrong.logs[0].kind = layer2_core::sandbox::AgentLogKind::Deleted;
    assert!(matches!(
        super::super::agent_worker_reconcile::reconcile_publication_after_unknown(
            &store, &wrong, at
        )
        .await,
        Err(SandboxStoreError::SequenceConflict)
    ));
    assert_eq!(receipt.agent.status, AgentStatus::Ready);
    client.database(&name).drop().await.unwrap();
}
