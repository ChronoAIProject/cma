use super::*;
use layer2_core::sandbox::agent_io::{
    AgentResponseStatus, CommandClaim, CommandDispatch, CommandLane,
};
use layer2_core::sandbox::{AgentOperationDelivery, AgentOperationKind};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn command_recovery_verifies_pending_approval_identity_and_original_source() {
    let (client, name) = database("segment_command_approval").await;
    segments::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = sandbox("sbx_segments");
    let (_, mut agent) = worktree_and_agent(&parent.id);
    let mut command =
        super::super::command_ledger_tests::request(&agent, &parent, "segment-command")
            .record(1, 2, None)
            .unwrap();
    command.status = AgentResponseStatus::InProgress;
    command.work.dispatch = Some(CommandDispatch {
        claim: CommandClaim {
            agent_id: agent.id.clone(),
            command_id: command.receipt.envelope.command_id.clone(),
            holder: "fixture".into(),
            fence: 1,
        },
        operation_sequence: 3,
        turn_sequence: 4,
        runtime_generation: 1,
        fenced_at: NOW.into(),
        delivery: AgentOperationDelivery::DispatchFenced,
    });
    command.validate().unwrap();
    agent.status = AgentStatus::Working;
    agent.conversation.conversation_id = Some("native-command".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.active_turn = Some(4);
    let kinds = [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-command".into(),
            runtime_generation: 1,
        },
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 3,
            operation: AgentOperationKind::SendMessage,
            state: AgentOperationDelivery::DispatchFenced,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 4,
            origin_key: Some(format!(
                "command:{}",
                command.receipt.envelope.command_id.as_str()
            )),
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
        AgentLogKind::ApprovalRequested {
            approval_id: "approval-fixture".into(),
            action: None,
            detail: None,
            runtime_locator: None,
        },
        AgentLogKind::ModeChanged {
            mode: "fixture".into(),
        },
    ];
    let source = kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let (agent, _) = install(&store, agent, source, 3).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let evidence = segments::command_evidence(&store, &agent, &command, &mut session)
        .await
        .unwrap()
        .unwrap();
    let mut recovered = command.clone();
    evidence
        .apply(&mut recovered, &mut CommandLane::default(), &agent)
        .unwrap();
    assert_eq!(recovered.status, AgentResponseStatus::WaitingAction);
    session.abort_transaction().await.unwrap();

    let filter = doc! {"agent_id":agent.id.as_str(),"kind":"approval"};
    let original = store
        .segment_logs
        .evidence
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    for update in [
        doc! {"$set":{"record.request_sequence":5_i64}},
        doc! {"$set":{"record.approval_id":"different"}},
        doc! {"$set":{"head_revision":99_i64}},
    ] {
        store
            .segment_logs
            .evidence
            .update_one(filter.clone(), update)
            .await
            .unwrap();
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        assert!(
            segments::command_evidence(&store, &agent, &command, &mut session)
                .await
                .is_err()
        );
        session.abort_transaction().await.unwrap();
        store
            .segment_logs
            .evidence
            .replace_one(filter.clone(), original.clone())
            .await
            .unwrap();
    }
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":6_i64},
            doc! {"$set":{"record.appended_at":"2026-09-06T00:00:00Z"}},
        )
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(
        segments::command_evidence(&store, &agent, &command, &mut session)
            .await
            .is_err()
    );
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}
