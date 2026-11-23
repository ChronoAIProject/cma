use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_rejects_stale_runtime_identity_and_every_incomplete_log_or_event_stream() {
    let (client, name, store) = database().await;
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_stale").await;
    for field in ["revision", "runtime_id", "runtime_version", "bound_at"] {
        let mut request = loss(&sandbox);
        match field {
            "revision" => request.expected_revision += 1,
            "runtime_id" => request
                .expected_runtime
                .as_mut()
                .unwrap()
                .runtime_id
                .push('x'),
            "runtime_version" => request
                .expected_runtime
                .as_mut()
                .unwrap()
                .runtime_version
                .push('x'),
            "bound_at" => request.expected_runtime.as_mut().unwrap().bound_at = OBSERVED.into(),
            _ => unreachable!(),
        }
        assert_eq!(
            store.recover_runtime_loss(request).await.unwrap(),
            SandboxRuntimeLossOutcome::Superseded
        );
    }
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    for terminal in [true, false] {
        let (sandbox, mut agent) =
            checkpoint_fixture(&store, &format!("sbx_corrupt_{terminal}")).await;
        if terminal {
            agent
                .transition(
                    AgentStatus::Stopped,
                    BEFORE.into(),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
            store
                .agents
                .update_one(
                    doc! { "_id": agent.id.as_str() },
                    doc! { "$set": agent_replacement_fields(&agent).unwrap() },
                )
                .await
                .unwrap();
        }
        store
            .agent_log
            .delete_one(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap();
        assert_eq!(
            store.recover_runtime_loss(loss(&sandbox)).await,
            Err(SandboxStoreError::SequenceConflict)
        );
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(sandbox.clone())
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert!(store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
    let (sandbox, agent) = checkpoint_fixture(&store, "sbx_corrupt_events").await;
    store
        .sandboxes
        .update_one(
            doc! { "_id": sandbox.id.as_str() },
            doc! { "$set": { "last_seq": 1_i64 } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.recover_runtime_loss(loss(&sandbox)).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_retries_a_concurrent_log_head_and_fences_terminal_agent_evidence() {
    for enabled in [false, true] {
        let (client, name, store) = database().await;
        let store = store.with_runtime_loss_bounded_proof(enabled);
        for terminal in [true, false] {
            let (sandbox, mut agent) =
                checkpoint_fixture(&store, &format!("sbx_race_{terminal}")).await;
            if terminal {
                agent
                    .transition(
                        AgentStatus::Stopped,
                        BEFORE.into(),
                        Some(AgentStopReason::Owner),
                    )
                    .unwrap();
                store
                    .agents
                    .update_one(
                        doc! { "_id": agent.id.as_str() },
                        doc! { "$set": agent_replacement_fields(&agent).unwrap() },
                    )
                    .await
                    .unwrap();
            }
            let mut session = client.start_session().await.unwrap();
            start_transaction(&mut session).await.unwrap();
            agents::load_agents(&store, &sandbox.id, &mut session)
                .await
                .unwrap();
            store
                .append_agent_log(
                    &agent.id,
                    OBSERVED.into(),
                    AgentLogKind::OwnerMessage {
                        content: "Accepted immediately before recovery.".into(),
                    },
                )
                .await
                .unwrap();
            let result = transaction::apply(&store, &loss(&sandbox), None, &mut session).await;
            assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
            let _ = session.abort_transaction().await;
            assert_eq!(
                store.recover_runtime_loss(loss(&sandbox)).await.unwrap(),
                SandboxRuntimeLossOutcome::RecoveryScheduled
            );
            let logs = store.agent_log(&agent.id, 0, 100).await.unwrap();
            assert!(
                matches!(&logs[1].kind, AgentLogKind::OwnerMessage { content } if content == "Accepted immediately before recovery.")
            );
        }
        client.database(&name).drop().await.unwrap();
    }
}
