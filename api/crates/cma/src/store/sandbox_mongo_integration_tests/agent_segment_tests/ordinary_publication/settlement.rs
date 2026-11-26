use super::*;

async fn finish(store: &MongoSandboxStore, current: &Agent) {
    DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn adopted_command_settlement_abort_and_corrupt_source_preserve_custody() {
    let (client, name, store, current, command) = working("v2_settlement_abort").await;
    finish(&store, &current).await;
    let id = &command.receipt.envelope.command_id;
    let before = snapshot(&client, &name).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let planned = crate::store::agent_commands::reconcile::apply(&store, id, &mut session)
        .await
        .unwrap_or_else(|_| panic!("segmented command evidence must settle"));
    assert_eq!(planned.status, AgentResponseStatus::Completed);
    session.abort_transaction().await.unwrap();
    assert_eq!(snapshot(&client, &name).await, before);
    let original = store
        .agents
        .find_one(doc! {"_id":current.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":current.id.as_str()},
            doc! {"$inc":{"agent_log_head_revision":1_i64}},
        )
        .await
        .unwrap();
    let corrupt = snapshot(&client, &name).await;
    assert_eq!(
        store.reconcile_command(id).await.unwrap_err(),
        CommandLedgerError::Corrupt
    );
    assert_eq!(snapshot(&client, &name).await, corrupt);
    store
        .agents
        .replace_one(doc! {"_id":current.id.as_str()}, original)
        .await
        .unwrap();
    assert_eq!(store.reconcile_command(id).await.unwrap(), planned);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn adopted_command_unknown_settlement_reopens_without_replaying_runtime_input() {
    for committed in [true, false] {
        let (admin, name, initial, current, command) = working("v2_settlement_unknown").await;
        finish(&initial, &current).await;
        let original = snapshot(&admin, &name).await;
        let before_logs = initial.agent_log(&current.id, 0, 100).await.unwrap();
        let (client, app, original_session) =
            super::uncertain::observed_client_for("settlement").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        super::uncertain::fail_commit(&admin, &app, committed).await;
        let result = store
            .reconcile_command(&command.receipt.envelope.command_id)
            .await;
        super::uncertain::clear(&admin).await;
        assert_eq!(
            result.unwrap_err(),
            CommandLedgerError::Unavailable,
            "committed={committed}"
        );
        let unresolved = snapshot(&admin, &name).await;
        if !committed {
            assert_eq!(unresolved, original);
        }
        super::uncertain::release_abandoned_transaction(&admin, &original_session).await;
        assert_eq!(snapshot(&admin, &name).await, unresolved);
        drop(store);
        client.shutdown().await;
        let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
        let settled = reopened
            .reconcile_command(&command.receipt.envelope.command_id)
            .await
            .unwrap_or_else(|error| panic!("restart settlement, committed={committed}: {error:?}"));
        assert_eq!(settled.status, AgentResponseStatus::Completed);
        assert_eq!(
            reopened.agent_log(&current.id, 0, 100).await.unwrap(),
            before_logs
        );
        let stable = snapshot(&admin, &name).await;
        assert_eq!(
            reopened
                .reconcile_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            settled
        );
        assert_eq!(snapshot(&admin, &name).await, stable);
        admin.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_audit_checks_segmented_terminal_in_one_read_only_snapshot() {
    let (client, name, store, current, admitted) = working("v2_task_audit").await;
    // The fixture returns the original queued admission, while its persisted
    // receipt has already acquired a dispatch fence. Preserve that real state.
    let row = store
        .agent_commands
        .find_one(doc! {"_id":admitted.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut command = crate::store::agent_commands::read_command(&row).unwrap();
    assert!(command.work.dispatch.is_some());
    store
        .agent_commands
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"record.receipt.admitted_at":1,"_id":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("agent_command_admission_cohort".to_string())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    command.receipt.admitted_at = "2026-01-01T00:00:00Z".into();
    let id = &command.receipt.envelope.command_id;
    store
        .agent_commands
        .replace_one(
            doc! {"_id":id.as_str()},
            crate::store::agent_commands::command_document(&command).unwrap(),
        )
        .await
        .unwrap();
    let selection = crate::config::task_audit::Selection {
        from: command.receipt.admitted_at.clone(),
        to: Some("2026-01-02T00:00:00Z".into()),
        overdue_seconds: 60,
    };
    let before = snapshot(&client, &name).await;
    let report = crate::store::task_audit::observe(&store, &selection)
        .await
        .unwrap();
    assert!(report
        .metrics()
        .contains("audit_tasks{window=\"release\",result=\"unfinished\"} 1\n"));
    assert_eq!(snapshot(&client, &name).await, before);
    finish(&store, &current).await;
    let before = snapshot(&client, &name).await;
    let report = crate::store::task_audit::observe(&store, &selection)
        .await
        .unwrap();
    assert!(report
        .metrics()
        .contains("audit_tasks{window=\"release\",result=\"status_mismatch\"} 1\n"));
    assert_eq!(snapshot(&client, &name).await, before);
    store.reconcile_command(id).await.unwrap();
    let before = snapshot(&client, &name).await;
    let report = crate::store::task_audit::observe(&store, &selection)
        .await
        .unwrap();
    assert!(report
        .metrics()
        .contains("audit_tasks{window=\"release\",result=\"verified_terminal\"} 1\n"));
    assert_eq!(snapshot(&client, &name).await, before);
    // Tamper only a source point. The observer must not trust the receipt or
    // derived projection alone, and may not quarantine or repair the source.
    store.agent_log.update_one(doc! {"agent_id":current.id.as_str(),"seq":command.work.dispatch.as_ref().unwrap().turn_sequence as i64}, doc! {"$set":{"record.appended_at":"2026-02-01T00:00:00Z"}}).await.unwrap();
    let before = snapshot(&client, &name).await;
    let report = crate::store::task_audit::observe(&store, &selection)
        .await
        .unwrap();
    assert!(report
        .metrics()
        .contains("audit_tasks{window=\"release\",result=\"evidence_invalid\"} 1\n"));
    assert_eq!(snapshot(&client, &name).await, before);
    client.database(&name).drop().await.unwrap();
}
