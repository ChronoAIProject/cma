use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn prepared_checkpoint_keeps_creation_identity_through_repeated_recovery() {
    use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD};
    use crate::store::agent_log_telemetry::{isolated_test as isolated, snapshot, ScanCaller};
    use layer2_core::sandbox::store::AgentLogStore;
    let (client, name, store, _, workspace, mut agent) = failed_fixture("bounded_recovery").await;
    let store = store
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let identity = parse_integrity(&row).unwrap().unwrap().history_identity;
    let original_worktree = agent.creation_worktree_id().clone();
    for key in ["first", "second"] {
        agent = recover(&store, &agent, key).await.unwrap();
        assert_ne!(&agent.worktree_id, &original_worktree);
        assert_eq!(agent.creation_worktree_id(), &original_worktree);
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        let checkpoint = parse_integrity(&row).unwrap().unwrap();
        assert_eq!(checkpoint.history_identity, identity);
        assert_eq!(checkpoint.bound_agent_revision, agent.revision);
        isolated(async {
            store
                .append_agent_log(
                    &agent.id,
                    NOW.into(),
                    AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::None,
                    },
                )
                .await
                .unwrap();
            assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
        })
        .await;
        agent = store.get_agent(&agent.id).await.unwrap().unwrap();
        agent = fail_retained(&store, agent, &workspace).await;
    }
    // A rollback-shaped revision writer preserves the additive block. Forward
    // upgrade explicitly audits the same original lineage before activation.
    agent.revision += 1;
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": crate::store::agents::agent_replacement_fields(&agent).unwrap() },
        )
        .await
        .unwrap();
    let restarted = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    let row = restarted
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(INTEGRITY_FIELD));
    assert_eq!(
        parse_integrity(&row).unwrap().unwrap().history_identity,
        identity
    );
    // Invalid retained lineage must refuse even when the envelope still exists.
    restarted
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": {
                "record.work.worktree_recoveries.0.original_worktree.workspace_id": "wsp_foreign"
            }},
        )
        .await
        .unwrap();
    assert!(restarted
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::None
            }
        )
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}
