use super::*;
use layer2_core::sandbox::{store::AgentLogStore, AgentLogKind};

#[tokio::test]
async fn task_activity_separates_maintenance_and_later_owner_turns() {
    let fixture = fixture(10);
    let sandbox = stored_sandbox("sbx_activity", "owner", SandboxStatus::Active, 4);
    let workspace = stored_workspace(&sandbox, "wks_activity");
    let tree = stored_worktree(&workspace, "wkt_activity", "agt_activity");
    let mut agent = stored_agent(&workspace, &tree, "agt_activity");
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some("activity-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    insert_hierarchy(
        &fixture.store,
        &sandbox,
        Some(&workspace),
        Some(&tree),
        Some(&agent),
    )
    .await;
    let yesterday = "2026-09-04T17:00:00Z";
    let today = "2026-09-05T09:00:00Z";
    for kind in [
        AgentLogKind::OwnerMessage {
            content: "first task".into(),
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 2,
            model: None,
            origin_key: None,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 2 },
    ] {
        fixture
            .store
            .append_agent_log(&agent.id, yesterday.into(), kind)
            .await
            .unwrap();
    }
    for _ in 0..32 {
        let entry = fixture
            .store
            .append_agent_log(
                &agent.id,
                today.into(),
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                },
            )
            .await
            .unwrap();
        agent = fixture.store.get_agent(&agent.id).await.unwrap().unwrap();
        let revision = agent.revision;
        agent.revision += 1;
        agent.updated_at = today.into();
        agent.conversation.last_seq = entry.seq;
        fixture
            .store
            .save_agent(revision, agent.clone())
            .await
            .unwrap();
    }
    let (status, body) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/workspaces/wks_activity", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agents"][0]["task_activity_at"], yesterday);
    assert_eq!(body["agents"][0]["updated_at"], today);
    assert_eq!(body["agents"][0]["task_status"], "completed");
    assert!(agent.revision > 32);
    for kind in [
        AgentLogKind::OwnerMessage {
            content: "second task".into(),
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 37,
            model: None,
            origin_key: None,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 37 },
    ] {
        fixture
            .store
            .append_agent_log(&agent.id, today.into(), kind)
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 38;
    fixture
        .store
        .save_agent(revision, agent.clone())
        .await
        .unwrap();
    let (status, body) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/workspaces/wks_activity", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["agents"][0]["task_activity_at"], today);
    let value = serde_json::to_value(&agent).unwrap();
    assert!(value.get("task_activity_at").is_none());
    assert!(value["conversation"].get("task_activity_at").is_none());
}

#[test]
fn task_activity_memory_allowlist_head_and_gap_contract() {
    use layer2_core::sandbox::{task_activity::from_log, AgentLogEntry, RecordedToolOutput};
    let sandbox = stored_sandbox("sbx_evidence", "owner", SandboxStatus::Active, 4);
    let workspace = stored_workspace(&sandbox, "wks_evidence");
    let tree = stored_worktree(&workspace, "wkt_evidence", "agt_evidence");
    let mut agent = stored_agent(&workspace, &tree, "agt_evidence");
    agent.conversation.last_seq = 20;
    let yesterday = "2026-09-04T17:00:00Z";
    let kinds = [
        AgentLogKind::OwnerMessage {
            content: "task".into(),
        },
        AgentLogKind::AgentMessage {
            content: "answer".into(),
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            model: None,
            origin_key: None,
        },
        AgentLogKind::ApprovalRequested {
            approval_id: "approval".into(),
            action: None,
            detail: None,
            runtime_locator: None,
        },
        AgentLogKind::ApprovalResolved {
            approval_id: "approval".into(),
            decision: "allow".into(),
        },
        AgentLogKind::ToolCallStarted {
            call_id: "call".into(),
            title: "tool".into(),
            kind: "read".into(),
            status: "in_progress".into(),
        },
        AgentLogKind::ToolCallUpdated {
            call_id: "call".into(),
            status: "completed".into(),
            title: None,
            output: None,
        },
        AgentLogKind::ToolOutputReported {
            call_id: "call".into(),
            output: RecordedToolOutput {
                text: "result".into(),
                truncated: false,
            },
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::TurnInterrupted { turn_sequence: 3 },
        AgentLogKind::TurnFailed {
            turn_sequence: 3,
            failure: layer2_core::sandbox::TypedFailure {
                class: "task_failed".into(),
                detail: "synthetic failure".into(),
            },
        },
    ];
    let mut entries = Vec::new();
    assert_eq!(from_log(&agent, &entries), None);
    for (index, kind) in kinds.into_iter().enumerate() {
        entries.push(AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: yesterday.into(),
            kind,
        });
        assert_eq!(from_log(&agent, &entries).as_deref(), Some(yesterday));
    }
    for (index, kind) in [
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection,
        },
        AgentLogKind::Usage {
            input_tokens: 1,
            output_tokens: 1,
            context_used: 1,
            context_window: None,
        },
        AgentLogKind::RuntimeGenerationRetired {
            previous_generation: 1,
            runtime_generation: 2,
        },
        AgentLogKind::RuntimeMetadataChanged {
            metadata: Default::default(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        entries.push(AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 12,
            appended_at: NOW.into(),
            kind,
        });
    }
    let mut future = entries[0].clone();
    future.seq = 21;
    future.appended_at = NOW.into();
    entries.push(future);
    assert_eq!(from_log(&agent, &entries).as_deref(), Some(yesterday));
    entries.push(AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 16,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationGap { frames: 1 },
    });
    assert_eq!(from_log(&agent, &entries), None);
    entries.last_mut().unwrap().seq = 4;
    assert_eq!(from_log(&agent, &entries), None);
    entries.pop();
    entries[10].agent_id = AgentId("foreign".into());
    assert_eq!(from_log(&agent, &entries), None);
    entries[10].agent_id = agent.id.clone();
    entries[10].kind = AgentLogKind::TurnCompleted { turn_sequence: 99 };
    assert_eq!(from_log(&agent, &entries), None);
    entries[10].kind = AgentLogKind::TurnCompleted { turn_sequence: 3 };
    entries[0].appended_at = NOW.into();
    assert_eq!(from_log(&agent, &entries).as_deref(), Some(yesterday));
    let started = entries.remove(2);
    assert_eq!(from_log(&agent, &entries), None);
    entries.insert(2, started);
    entries[10].appended_at = "invalid".into();
    assert_eq!(from_log(&agent, &entries), None);
}
