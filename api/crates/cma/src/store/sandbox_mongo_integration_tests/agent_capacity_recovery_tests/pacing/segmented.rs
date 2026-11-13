//! Exercise both real discovery arms after a keyed publication adopts v2.
use super::*;
use crate::store::agent_log_segments::{schema, writer_gate};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

async fn adopt(store: &MongoSandboxStore, agent: &Agent) -> Agent {
    let db = store.client.database(&store.agents.namespace().db);
    schema::migrate(&db).await.unwrap();
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    writer_gate::replace(
        &db,
        writer_gate::GateName::Segments,
        0,
        writer_gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    store
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: "pacing-adoption".into(),
                fingerprint: [7; 32],
                appended_at: stamp(30),
            },
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert!(crate::store::agent_log_segments::read_format(&row)
        .unwrap()
        .is_some());
    store.get_agent(&agent.id).await.unwrap().unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_due_work_discovers_adopted_ordinary_and_paced_agents() {
    let (client, name) = database("capacity_due_segmented").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let legacy = ready_conversation_fixture(&store, "legacy-due").await;
    let original = fixture::ready_with_created(&store, "segmented-due").await;
    failure(&store, &original.id, 0, PARENT).await;
    failure(&store, &original.id, 30, PARENT).await;
    let adopted = adopt(&store, &original).await;
    let mut expected = vec![legacy.id.clone(), adopted.id.clone()];
    expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    // Rebuild the advisory hint on an adopted record through the production
    // post-publication CAS. No aggregate or format field may be rewritten.
    store
        .agents
        .update_one(
            doc! {"_id": adopted.id.as_str()},
            doc! {"$unset": {FIELD: ""}},
        )
        .await
        .unwrap();
    let before = store
        .agents
        .find_one(doc! {"_id": adopted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let sandbox = store
        .get_sandbox(&adopted.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = store
        .get_workspace(&adopted.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&adopted.worktree_id)
        .await
        .unwrap()
        .unwrap();
    crate::store::agent_failure_pacing::record(
        &store,
        &AgentWorkMutation {
            expected_agent_revision: adopted.revision - 1,
            holder: "worker".into(),
            fence: adopted.work.next_fence - 1,
            agent: adopted.clone(),
            appended_at: stamp(30),
            worktree: None,
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::PublishConnection,
                failure: TypedFailure {
                    class: PARENT.into(),
                    detail: "fixture".into(),
                },
            }],
        },
        &AgentRetryObservation {
            agent_revision: adopted.revision - 1,
            command_admission_generation: 0,
            sandbox_revision: sandbox.revision,
            workspace_revision: workspace.revision,
            worktree_revision: worktree.revision,
        },
    )
    .await
    .unwrap();
    let mut after = store
        .agents
        .find_one(doc! {"_id": adopted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.get_document(FIELD).unwrap().get_str("until").unwrap(),
        stamp(60)
    );
    after.remove(FIELD);
    assert_eq!(before, after);
    assert_eq!(
        store.due_agent_work(&stamp(31), 10).await.unwrap(),
        vec![legacy.id.clone()]
    );

    // Parent progress wakes the future-paced arm before work_ready_at. The
    // unrelated v1 parent predicates still retain their original schema.
    store
        .sandboxes
        .update_one(
            doc! {"_id": adopted.sandbox_id.as_str()},
            doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(31), 10).await.unwrap(),
        expected
    );
    assert_eq!(
        store.due_agent_work(&stamp(31), 1).await.unwrap(),
        expected[..1]
    );
    assert_eq!(
        store.get_agent(&adopted.id).await.unwrap(),
        Some(adopted.clone())
    );

    // Expired pacing uses the ordinary arm. Neither discovery arm changes the
    // adopted envelope, aggregate, retained history or input receipts.
    let before = store
        .agents
        .find_one(doc! {"_id": adopted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(60), 10).await.unwrap(),
        expected
    );
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": adopted.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );

    // A discovered ID grants no lease. Malformed v2 fails before parent/CAS
    // writes; it cannot select the legacy path or downgrade the envelope.
    store
        .agents
        .update_one(
            doc! {"_id": adopted.id.as_str()},
            doc! {"$set": {"agent_log_incarnation": "invalid"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_agent_work(&adopted.id, "claim", stamp(60), stamp(120))
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    let corrupt = store
        .agents
        .find_one(doc! {"_id": adopted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(corrupt.get_i64("document_schema_version").unwrap(), 2);
    assert_eq!(corrupt.get("record"), before.get("record"));
    client.database(&name).drop().await.unwrap();
}
