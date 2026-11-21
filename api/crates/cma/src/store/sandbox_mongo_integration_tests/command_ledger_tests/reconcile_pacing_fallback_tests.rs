use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn malformed_or_absent_agent_heads_fall_back_to_authoritative_errors() {
    let (client, name) = database("reconcile_invalid_head").await;
    let (store, agent, command, execution) = working(&client, &name, 1_000, false).await;
    let original = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let worker = AgentCommandWorker::with_recheck_delay(
        store.clone(),
        execution,
        "fallback".into(),
        Duration::from_secs(1),
    )
    .unwrap();
    for mutation in [
        doc! { "$unset": { "record.conversation.last_seq": "" } },
        doc! { "$set": { "record.conversation.last_seq": "invalid" } },
        doc! { "$set": { "record.conversation.runtime_generation": -1_i64 } },
        doc! { "$set": { "record.conversation.active_turn": "invalid" } },
        doc! { "$set": { "document_schema_version": 9_999_i64 } },
    ] {
        store
            .agents
            .update_one(doc! {"_id": agent.id.as_str()}, mutation)
            .await
            .unwrap();
        assert!(store
            .command_reconcile_heads(std::slice::from_ref(&agent.id))
            .await
            .unwrap()
            .is_empty());
        let discovery = worker
            .discover_scheduled(&PageRequest::new(50, None).unwrap())
            .await
            .unwrap();
        assert_eq!(discovery.page.items.len(), 1);
        assert!(!discovery.page.items[0].reconcile_current);
        assert_eq!(
            worker
                .run_scheduled(&command.receipt.envelope.command_id)
                .await,
            Err(CommandLedgerError::Corrupt)
        );
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap(),
            command
        );
        store
            .agents
            .replace_one(doc! {"_id": agent.id.as_str()}, original.clone())
            .await
            .unwrap();
    }
    store
        .agents
        .delete_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap();
    assert!(store
        .command_reconcile_heads(std::slice::from_ref(&agent.id))
        .await
        .unwrap()
        .is_empty());
    let discovery = worker
        .discover_scheduled(&PageRequest::new(50, None).unwrap())
        .await
        .unwrap();
    assert!(!discovery.page.items[0].reconcile_current);
    assert_eq!(
        worker
            .run_scheduled(&command.receipt.envelope.command_id)
            .await,
        Err(CommandLedgerError::NotFound)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stopped_and_replaced_runtimes_still_settle_lost_with_both_admission_modes() {
    for bounded in [false, true] {
        for replaced in [false, true] {
            let (client, name) = database("reconcile_runtime_lost").await;
            let (store, agent, command, execution) = working(&client, &name, 1_000, bounded).await;
            let mut changed = store.get_agent(&agent.id).await.unwrap().unwrap();
            let revision = changed.revision;
            if replaced {
                changed.conversation.runtime_generation = Some(99);
            } else {
                changed.status = AgentStatus::Stopped;
            }
            // Use the revision-checked writer so a generation change invalidates
            // the old trusted checkpoint instead of corrupting its bound revision.
            changed.revision += 1;
            store.save_agent(revision, changed).await.unwrap();
            let worker = AgentCommandWorker::with_recheck_delay(
                store.clone(),
                execution,
                "lost".into(),
                Duration::from_secs(1),
            )
            .unwrap();
            let discovery = worker
                .discover_scheduled(&PageRequest::new(50, None).unwrap())
                .await
                .unwrap();
            assert!(!discovery.page.items[0].reconcile_current);
            assert_eq!(
                worker
                    .run_scheduled(&command.receipt.envelope.command_id)
                    .await
                    .unwrap(),
                Duration::ZERO
            );
            let settled = store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(settled.status, AgentResponseStatus::Failed);
            assert_eq!(
                settled.work.failure.unwrap().code,
                CommandFailureCode::RuntimeLost
            );
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_heads_timeout_returns_unavailable_and_does_not_change_the_command() {
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    let app = format!("reconcile-heads-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_reconcile_timeout_{}", uuid::Uuid::new_v4().simple());
    let (store, agent, command, _) = working(&client, &name, 1_000, false).await;
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {"failCommands": ["find"], "appName": app, "errorCode": 50_i32}
        })
        .await
        .unwrap();
    let result = store
        .command_reconcile_heads(std::slice::from_ref(&agent.id))
        .await;
    client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(CommandLedgerError::Unavailable));
    assert_eq!(
        store
            .reconcile_command(&command.receipt.envelope.command_id)
            .await
            .unwrap(),
        command
    );
    client.database(&name).drop().await.unwrap();
}
