//! Exercise the retained GET root after real gated adoption, without mutation.
use super::*;
use crate::store::agent_log_segments::{publication, writer_gate as gate};
use crate::store::sandbox_mongo::{commit_transaction, CommitOutcome};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

pub(super) async fn fixture(suffix: &str) -> (Client, String, MongoSandboxStore, Agent) {
    let (client, name) = database(suffix).await;
    let db = client.database(&name);
    segments::schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_source(&store, suffix).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    gate::replace(
        &db,
        gate::GateName::Segments,
        0,
        gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let kind = AgentLogKind::ConversationGap { frames: 1 };
    let identity = AgentLogPublicationIdentity {
        key: "retained-assessment-adoption".into(),
        appended_at: NOW.into(),
        fingerprint: publication::request_digest(&kind)
            .unwrap()
            .bytes
            .try_into()
            .unwrap(),
    };
    store
        .append_agent_log_keyed(&agent.id, &identity, kind)
        .await
        .unwrap();
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    (client, name, store, agent)
}

// The general worker fixture starts at its open event. Adoption additionally
// requires the original Created row, so build this retained source through the
// actual legacy append and worker publication boundaries.
pub(super) async fn ready_source(store: &MongoSandboxStore, suffix: &str) -> Agent {
    let (_, mut worktree, claimed) = claimed_agent_work_fixture(store, suffix).await;
    store
        .append_agent_log(
            &claimed.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: claimed.workspace_id.clone(),
                worktree_id: claimed.creation_worktree_id().clone(),
            },
        )
        .await
        .unwrap();
    let claimed = store.get_agent(&claimed.id).await.unwrap().unwrap();
    let conversation_id = format!("retained-conversation-{suffix}");
    let mut claimed = ready_publication_claim(claimed, &conversation_id);
    // Preparing the runtime conversation changes semantic state before its
    // publication. Advance the revision through the normal store boundary so
    // the previous Created checkpoint is stale, never falsely current.
    let expected_revision = claimed.revision;
    claimed.revision += 1;
    store
        .save_agent(expected_revision, claimed.clone())
        .await
        .unwrap();
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
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: ready.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
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

async fn rows(store: &MongoSandboxStore, agent: &Agent) -> Vec<Document> {
    let mut result = Vec::new();
    for (collection, id) in [
        (&store.agents, agent.id.as_str()),
        (&store.workspaces, agent.workspace_id.as_str()),
        (&store.sandboxes, agent.sandbox_id.as_str()),
    ] {
        result.push(
            collection
                .find_one(doc! {"_id": id})
                .await
                .unwrap()
                .unwrap(),
        );
    }
    result
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_assessment_reads_segmented_history_and_rejects_corruption_without_writes() {
    let (client, name, store, agent) = fixture("retained_segmented").await;
    let before = rows(&store, &agent).await;
    let snapshot = store
        .retained_runtime_recovery_snapshot(&agent.sandbox_id)
        .await
        .unwrap();
    assert_eq!(snapshot.agents, vec![agent.clone()]);
    assert_eq!(snapshot.workspaces.len(), 1);
    assert_eq!(rows(&store, &agent).await, before);
    // A malformed v2 envelope cannot select the legacy reader or create an
    // audit/quarantine write as a side effect of the assessment.
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"agent_log_incarnation": "mismatched-incarnation"}},
        )
        .await
        .unwrap();
    let corrupt = rows(&store, &agent).await;
    assert!(store
        .retained_runtime_recovery_snapshot(&agent.sandbox_id)
        .await
        .is_err());
    assert_eq!(rows(&store, &agent).await, corrupt);
    assert_eq!(
        store.agent_log.count_documents(doc! {}).await.unwrap(),
        agent.conversation.last_seq
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_assessment_refuses_oversized_workspaces_before_mutation() {
    let (client, name, store, agent) = fixture("retained_child_bound").await;
    let original = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let before = rows(&store, &agent).await;
    let mut children = Vec::new();
    for index in 0..512 {
        let mut workspace = original.clone();
        workspace.id = WorkspaceId(format!("retained-workspace-{index:04}"));
        workspace.name = format!("retained-workspace-{index:04}");
        workspace.path = format!("/home/cma/{}", workspace.name);
        children.push(crate::store::workspaces::workspace_document(&workspace).unwrap());
    }
    store.workspaces.insert_many(children).await.unwrap();
    assert!(matches!(
        store
            .retained_runtime_recovery_snapshot(&agent.sandbox_id)
            .await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert_eq!(rows(&store, &agent).await, before);
    assert_eq!(
        store.workspaces.count_documents(doc! {}).await.unwrap(),
        513
    );
    client.database(&name).drop().await.unwrap();
}
