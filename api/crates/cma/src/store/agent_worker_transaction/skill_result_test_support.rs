use super::*;
use crate::store::agent_log_segments::{schema, writer_control, writer_gate};
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{
    AgentSkillResultGuard, AgentStore, SandboxStore, WorkspaceStore, WorktreeStore,
};
use layer2_core::sandbox::{WorkspaceStatus, WorktreeStatus};
use std::collections::BTreeMap;

pub(crate) async fn enable_segments(store: &MongoSandboxStore, agent: &Agent) {
    let db = store.client.database(&store.agents.namespace().db);
    schema::migrate(&db).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&db).await;
    writer_control::execute(
        &store.client,
        &db,
        writer_gate::GateName::Segments,
        writer_control::Action::Replace {
            expected_revision: 0,
            policy: writer_gate::Policy {
                enabled: true,
                agents: vec![agent.id.to_string()],
                ..Default::default()
            },
        },
    )
    .await
    .unwrap();
}

pub(crate) async fn assert_format(
    store: &MongoSandboxStore,
    id: &layer2_core::AgentId,
    segmented: bool,
) {
    let row = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::store::agent_log_segments::read_format(&row)
            .unwrap()
            .is_some(),
        segmented
    );
}

pub(crate) async fn snapshot(store: &MongoSandboxStore) -> BTreeMap<String, Vec<Document>> {
    let db = store.client.database(&store.agents.namespace().db);
    let mut result = BTreeMap::new();
    for collection in [
        "sandboxes",
        "workspaces",
        "worktrees",
        "agents",
        "agent_log",
        "sandbox_events",
    ]
    .into_iter()
    .chain(
        crate::store::agent_log_segments::COLLECTIONS
            .iter()
            .copied(),
    ) {
        result.insert(
            collection.to_owned(),
            db.collection::<Document>(collection)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}

pub(crate) async fn change_parent(store: &MongoSandboxStore, agent: &Agent, which: &str) {
    match which {
        "sandbox" | "runtime" => {
            let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
            let revision = parent.revision;
            if which == "sandbox" {
                parent
                    .begin_pause(layer2_core::now_rfc3339(), None)
                    .unwrap();
            } else {
                parent.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
                parent.revision += 1;
            }
            store.save_sandbox(revision, parent).await.unwrap();
        }
        "workspace" => {
            let mut parent = store
                .get_workspace(&agent.workspace_id)
                .await
                .unwrap()
                .unwrap();
            let revision = parent.revision;
            parent.status = WorkspaceStatus::Removing;
            parent.revision += 1;
            store.save_workspace(revision, parent).await.unwrap();
        }
        "worktree" => {
            let mut parent = store
                .get_worktree(&agent.worktree_id)
                .await
                .unwrap()
                .unwrap();
            let revision = parent.revision;
            parent.status = WorktreeStatus::Removing;
            parent.revision += 1;
            store.save_worktree(revision, parent).await.unwrap();
        }
        _ => panic!("unknown parent"),
    }
}

pub(crate) async fn snapshot_conflict(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    guard: &AgentSkillResultGuard,
    which: &str,
) -> Result<(), SandboxStoreError> {
    let mut session = store.client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    scope
        .find_one(
            &store.sandboxes,
            doc! {"_id":mutation.agent.sandbox_id.as_str()},
        )
        .await
        .unwrap()
        .unwrap();
    change_parent(store, &mutation.agent, which).await;
    let before = snapshot(store).await;
    let result = apply_publication_observed(
        store,
        mutation,
        None,
        None,
        &mut publication_history::AttemptHistory::default(),
        PublicationGuards {
            skill_result: Some(guard),
            ..Default::default()
        },
        scope,
    )
    .await;
    let _ = session.abort_transaction().await;
    assert_eq!(
        snapshot(store).await,
        before,
        "snapshot refusal must roll back every write"
    );
    assert!(
        matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)),
        "a parent write must invalidate the stale publication snapshot"
    );
    Err(SandboxStoreError::Conflict)
}

pub(crate) async fn rollback_and_reconcile_after_pause(
    store: &MongoSandboxStore,
    mutation: &AgentWorkMutation,
    guard: &AgentSkillResultGuard,
) -> Result<(), SandboxStoreError> {
    let before = snapshot(store).await;
    let mut session = store.client.start_session().await.unwrap();
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = apply_publication_observed(
        store,
        mutation,
        None,
        None,
        &mut publication_history::AttemptHistory::default(),
        PublicationGuards {
            skill_result: Some(guard),
            ..Default::default()
        },
        scope,
    )
    .await
    .unwrap_or_else(|_| panic!("valid original publication"));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        snapshot(store).await,
        before,
        "aborting must undo parent fences and all publication state"
    );
    assert!(
        !crate::store::agent_worker_reconcile::reconcile_publication(store, &receipt)
            .await
            .unwrap()
    );
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let receipt = apply_publication_observed(
        store,
        mutation,
        None,
        None,
        &mut publication_history::AttemptHistory::default(),
        PublicationGuards {
            skill_result: Some(guard),
            ..Default::default()
        },
        scope,
    )
    .await
    .unwrap_or_else(|_| panic!("fresh original publication"));
    session.commit_transaction().await.unwrap();
    let committed_at = session.operation_time().unwrap();
    change_parent(store, &mutation.agent, "sandbox").await;
    assert!(
        crate::store::agent_worker_reconcile::reconcile_publication_after_unknown(
            store,
            &receipt,
            committed_at
        )
        .await
        .unwrap(),
        "a later parent transition cannot invalidate the original committed receipt"
    );
    let saved = store.get_agent(&mutation.agent.id).await.unwrap().unwrap();
    assert_eq!(saved, mutation.agent);
    Ok(())
}

pub(crate) async fn remove_parent(store: &MongoSandboxStore, agent: &Agent, which: &str) {
    let (collection, id) = match which {
        "sandbox" => (&store.sandboxes, agent.sandbox_id.as_str()),
        "workspace" => (&store.workspaces, agent.workspace_id.as_str()),
        "worktree" => (&store.worktrees, agent.worktree_id.as_str()),
        _ => panic!("unknown parent"),
    };
    assert_eq!(
        collection
            .delete_one(doc! {"_id": id})
            .await
            .unwrap()
            .deleted_count,
        1
    );
}
