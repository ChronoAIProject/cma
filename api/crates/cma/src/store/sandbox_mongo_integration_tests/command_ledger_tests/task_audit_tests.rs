use super::*;
use crate::config::task_audit::Selection;
use crate::store::task_audit;

async fn index(store: &MongoSandboxStore) {
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
}
fn selection() -> Selection {
    Selection {
        from: NOW.into(),
        to: Some("2026-11-12T00:00:00Z".into()),
        overdue_seconds: 60,
    }
}
async fn snapshot(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    let mut snapshots = Vec::new();
    for collection in [
        &store.agents,
        &store.agent_commands,
        &store.agent_command_orders,
        &store.agent_log,
    ] {
        snapshots.push(
            collection
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    snapshots
}
fn metric(text: &str, name: &str, labels: &str, count: u64) {
    assert!(
        text.contains(&format!(
            "cma_task_cohort_{name}{{window=\"release\"{labels}}} {count}\n"
        )),
        "{text}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_audit_cutoff_replays_age_and_receipt_terminals_are_read_only() {
    let (client, name) = database("task_audit_cutoff").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    index(&store).await;
    let request = request(&agent, &parent, "task-audit-original");
    let command = record(store.reserve_command(request.clone()).await.unwrap());
    assert_eq!(
        command,
        record(store.reserve_command(request).await.unwrap())
    );
    let before = snapshot(&store).await;
    let report = task_audit::observe(&store, &selection()).await.unwrap();
    let text = report.metrics();
    metric(&text, "accepted_tasks", "", 1);
    metric(&text, "audit_tasks", ",result=\"unfinished\"", 1);
    metric(&text, "overdue_tasks", "", 1);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["exceptions"][0]["command_id"],
        command.receipt.envelope.command_id.as_str()
    );
    assert!(!text.contains(agent.id.as_str()));
    assert_eq!(snapshot(&store).await, before);
    // Explicit query uses handles only; a restart keeps the exact same cohort.
    let handles = MongoSandboxStore::audit_handles(&client, &name);
    metric(
        &task_audit::observe(&handles, &selection())
            .await
            .unwrap()
            .metrics(),
        "accepted_tasks",
        "",
        1,
    );
    let mut outside = selection();
    outside.from = "2026-11-12T00:00:00Z".into();
    outside.to = Some("2026-11-13T00:00:00Z".into());
    metric(
        &task_audit::observe(&store, &outside)
            .await
            .unwrap()
            .metrics(),
        "accepted_tasks",
        "",
        0,
    );
    // A durable typed refusal before dispatch is a public receipt terminal,
    // not falsely advertised as an independently checked native terminal.
    let mut refused = command;
    refused.status = AgentResponseStatus::Failed;
    refused.revision += 1;
    refused.work.failure = Some(CommandFailure {
        code: CommandFailureCode::AuthorityRevoked,
        at: NOW.into(),
    });
    store
        .agent_commands
        .replace_one(
            doc! {"_id":refused.receipt.envelope.command_id.as_str()},
            crate::store::agent_commands::command_document(&refused).unwrap(),
        )
        .await
        .unwrap();
    metric(
        &task_audit::observe(&store, &selection())
            .await
            .unwrap()
            .metrics(),
        "audit_tasks",
        ",result=\"receipt_terminal\"",
        1,
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_audit_cross_checks_terminal_and_reports_missing_or_wrong_evidence_without_repair() {
    let (client, name) = database("task_audit_terminal").await;
    let (store, _, _) = execution_tests::request_budget::completed(&client, &name).await;
    index(&store).await;
    let row = store
        .agent_commands
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let mut command = crate::store::agent_commands::read_command(&row).unwrap();
    command.receipt.admitted_at = NOW.into();
    // Settlement may lag public durable publication. Bring the receipt up to
    // its proven terminal once, before the observation under test.
    command = store
        .reconcile_command(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    command.receipt.admitted_at = NOW.into();
    store
        .agent_commands
        .replace_one(
            doc! {"_id":command.receipt.envelope.command_id.as_str()},
            crate::store::agent_commands::command_document(&command).unwrap(),
        )
        .await
        .unwrap();
    let before = snapshot(&store).await;
    let report = task_audit::observe(&store, &selection()).await.unwrap();
    metric(
        &report.metrics(),
        "audit_tasks",
        ",result=\"verified_terminal\"",
        1,
    );
    assert_eq!(snapshot(&store).await, before);
    command.status = AgentResponseStatus::Cancelled;
    command.work.failure = None;
    store
        .agent_commands
        .replace_one(
            doc! {"_id":command.receipt.envelope.command_id.as_str()},
            crate::store::agent_commands::command_document(&command).unwrap(),
        )
        .await
        .unwrap();
    let wrong = snapshot(&store).await;
    let report = task_audit::observe(&store, &selection()).await.unwrap();
    metric(
        &report.metrics(),
        "audit_tasks",
        ",result=\"status_mismatch\"",
        1,
    );
    metric(&report.metrics(), "classification_complete", "", 0);
    metric(&report.metrics(), "overdue_tasks", "", 1);
    assert_eq!(snapshot(&store).await, wrong);
    store
        .agents
        .delete_one(doc! {"_id":command.receipt.envelope.agent_id.as_str()})
        .await
        .unwrap();
    metric(
        &task_audit::observe(&store, &selection())
            .await
            .unwrap()
            .metrics(),
        "audit_tasks",
        ",result=\"evidence_missing\"",
        1,
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_audit_reuses_one_legacy_scan_for_commands_from_the_same_agent() {
    use crate::store::mongo_read_telemetry;
    use layer2_core::sandbox::{AgentUpdateRecorder, DurableAgentUpdateRecorder};
    use layer3_adapter::{
        ConversationInputSender, ConversationUpdate, PromptResponse, TurnStopReason,
    };

    let (client, name) = database("task_audit_shared_legacy").await;
    let sender = ConversationInputSender::spawn(4, |_| async { Ok(()) });
    let (store, agent, parent, execution, _) =
        execution_tests::composed_with_input(&client, &name, sender, "audit-fixture".into()).await;
    index(&store).await;
    let worker = AgentCommandWorker::new(store.clone(), execution, "audit-worker".into()).unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let second_admission = "2026-09-05T00:00:02Z";
    for (key, admitted_at) in [("audit-first", NOW), ("audit-second", second_admission)] {
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        let admitted = record(
            store
                .reserve_command(work_tests::live_request(&current, &parent, key))
                .await
                .unwrap(),
        );
        let command_id = &admitted.receipt.envelope.command_id;
        worker.run_one(command_id).await.unwrap();
        let working = store.get_agent(&agent.id).await.unwrap().unwrap();
        recorder
            .record_update(
                &agent.id,
                working.conversation.runtime_generation.unwrap(),
                &mut layer2_core::sandbox::AgentMessageAssembly::default(),
                ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let mut settled = store.reconcile_command(command_id).await.unwrap();
        settled.receipt.admitted_at = admitted_at.into();
        store
            .agent_commands
            .replace_one(
                doc! {"_id": command_id.as_str()},
                crate::store::agent_commands::command_document(&settled).unwrap(),
            )
            .await
            .unwrap();
    }
    let before = snapshot(&store).await;
    let selected = selection();
    let mut first = selected.clone();
    first.to = Some(second_admission.into());
    let mut expected_reads = None;
    for (cohort, count) in [(&first, 1), (&selected, 2)] {
        let (report, reads) = mongo_read_telemetry::isolated_test(async {
            let report = task_audit::observe(&store, cohort).await.unwrap();
            (report, mongo_read_telemetry::snapshot("agent_log"))
        })
        .await;
        metric(&report.metrics(), "accepted_tasks", "", count);
        metric(
            &report.metrics(),
            "audit_tasks",
            ",result=\"verified_terminal\"",
            count,
        );
        assert!(reads[0] > 0);
        if let Some(expected_reads) = expected_reads {
            assert_eq!(reads, expected_reads);
        }
        expected_reads = Some(reads);
    }
    assert_eq!(snapshot(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
