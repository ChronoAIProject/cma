use super::*;

async fn pending() -> (Arc<InMemorySandboxStore>, Agent) {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut agent = insert_hierarchy(
        &store,
        "agent",
        "worktree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let previous = agent.revision;
    agent.conversation.conversation_id = Some("durably-opened-native-id".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq += 1;
    agent.work.step = AgentWorkStep::PublishReady;
    agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", agent.id),
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    });
    agent.revision += 1;
    store.save_agent(previous, agent.clone()).await.unwrap();
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::WorkStepCompleted {
                step: AgentWorkStep::OpenConversation,
            },
        )
        .await
        .unwrap();
    (store, agent)
}

#[tokio::test]
async fn unpublished_open_reads_existing_history_without_inventing_conversation_or_turn() {
    for status in [
        AgentStatus::Starting,
        AgentStatus::Failed,
        AgentStatus::Stopped,
        AgentStatus::Deleted,
    ] {
        let (store, mut agent) = pending().await;
        let previous = agent.revision;
        agent.status = status;
        if status != AgentStatus::Starting {
            agent.work.step = AgentWorkStep::None;
        }
        agent.revision += 1;
        store.save_agent(previous, agent.clone()).await.unwrap();
        let reads = service(store.clone());
        let logs = reads
            .log(&caller("owner", true), &agent.id, 0, 100)
            .await
            .unwrap();
        assert_eq!(logs.len(), 2);
        assert!(!logs.iter().any(|entry| matches!(
            entry.kind,
            AgentLogKind::ConversationOpened { .. } | AgentLogKind::TurnStarted { .. }
        )));
        assert!(reads
            .transcript(&caller("owner", true), &agent.id)
            .await
            .unwrap()
            .turns
            .is_empty());
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    }
}

#[tokio::test]
async fn unpublished_identity_requires_matching_open_evidence_and_cannot_hide_missing_ready_marker()
{
    for change in [
        "receipt",
        "operation",
        "hash",
        "timestamp",
        "generation",
        "ready",
        "step",
        "duplicate",
        "log",
    ] {
        let (store, mut agent) = pending().await;
        let previous = agent.revision;
        match change {
            "receipt" => agent.work.receipts.clear(),
            "operation" => agent.work.receipts[0].operation_id = "agent:other:open:v1".into(),
            "hash" => agent.work.receipts[0].request_sha256 = "invalid".into(),
            "timestamp" => agent.work.receipts[0].completed_at = "2026-09-06T00:00:00Z".into(),
            "generation" => agent.conversation.runtime_generation = Some(2),
            "ready" => {
                agent.status = AgentStatus::Ready;
                agent.ready_at = Some(NOW.into());
            }
            "step" => agent.work.step = AgentWorkStep::OpenConversation,
            "duplicate" => agent.work.receipts.push(agent.work.receipts[0].clone()),
            "log" => {
                let mut log = store.agent_log(&agent.id, 0, 100).await.unwrap();
                log[1].kind = AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::MaterializeHome,
                };
                store.replace_agent_log_for_test(&agent.id, log);
            }
            _ => unreachable!(),
        }
        agent.revision += 1;
        store.save_agent(previous, agent.clone()).await.unwrap();
        assert!(
            matches!(
                service(store)
                    .log(&caller("owner", true), &agent.id, 0, 100)
                    .await,
                Err(AgentReadError::Corrupt(_))
            ),
            "{change}"
        );
    }
}
