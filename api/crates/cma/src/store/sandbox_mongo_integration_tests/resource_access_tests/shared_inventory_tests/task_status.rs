use super::*;
use layer2_core::sandbox::{AgentTaskStatus, HierarchyReadStore};

pub(super) async fn assert_authoritative_status_and_ready_evidence(
    store: &MongoSandboxStore,
    target: &ResourceTarget,
) {
    let original = store
        .resource_access_snapshot(target)
        .await
        .unwrap()
        .lineage
        .agent
        .unwrap();
    let mut agent = original.clone();
    agent.conversation.last_seq += 1;
    let completed = log_document(&AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: agent.conversation.last_seq,
        appended_at: NOW.into(),
        kind: AgentLogKind::TurnCompleted { turn_sequence: 1 },
    })
    .unwrap();
    let log_id = completed.get_str("_id").unwrap().to_owned();
    store.agent_log.insert_one(completed.clone()).await.unwrap();
    save_fixture_agent(store, &agent).await;
    assert_status(store, &agent, AgentTaskStatus::Completed).await;

    for corrupt_command in [false, true] {
        if corrupt_command {
            store
                .agent_commands
                .insert_one(doc! {"_id":"invalid-task-command", "agent_id":agent.id.as_str(), "status":"queued"})
                .await
                .unwrap();
        } else {
            store
                .agent_log
                .update_one(
                    doc! {"_id":&log_id},
                    doc! {"$set":{"record.agent_id":"foreign-agent"}},
                )
                .await
                .unwrap();
        }
        agent.status = AgentStatus::Ready;
        save_fixture_agent(store, &agent).await;
        assert!(matches!(
            store.read_workspace_hierarchy(&agent.workspace_id).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        for (status, expected) in [
            (AgentStatus::Starting, AgentTaskStatus::Starting),
            (AgentStatus::Working, AgentTaskStatus::Working),
            (
                AgentStatus::WaitingApproval,
                AgentTaskStatus::WaitingApproval,
            ),
            (AgentStatus::Paused, AgentTaskStatus::Paused),
            (AgentStatus::Stopped, AgentTaskStatus::Stopped),
            (AgentStatus::Failed, AgentTaskStatus::Failed),
        ] {
            agent.status = status;
            save_fixture_agent(store, &agent).await;
            assert_status(store, &agent, expected).await;
        }
        if corrupt_command {
            store
                .agent_commands
                .delete_one(doc! {"_id":"invalid-task-command"})
                .await
                .unwrap();
        } else {
            store
                .agent_log
                .replace_one(doc! {"_id":&log_id}, completed.clone())
                .await
                .unwrap();
        }
    }
    agent.status = AgentStatus::Ready;
    save_fixture_agent(store, &agent).await;
    assert_status(store, &agent, AgentTaskStatus::Completed).await;
    save_fixture_agent(store, &original).await;
    store
        .agent_log
        .delete_one(doc! {"_id":log_id})
        .await
        .unwrap();
}

async fn save_fixture_agent(store: &MongoSandboxStore, agent: &Agent) {
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(agent).unwrap(),
        )
        .await
        .unwrap();
}

async fn assert_status(store: &MongoSandboxStore, agent: &Agent, expected: AgentTaskStatus) {
    let hierarchy = store
        .read_workspace_hierarchy(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hierarchy.task_statuses.get(&agent.id), Some(&expected));
    let inventory = store
        .scan_shared_agent_provenance_inventory(
            &SubjectId::new("recipient"),
            &[SubjectId::new("org")],
            &AgentListQuery::default(),
            &PageRequest::default(),
        )
        .await
        .unwrap();
    let task = inventory
        .iter()
        .find(|item| item.provenance.resource.lineage.agent.as_ref().unwrap().id == agent.id)
        .unwrap();
    assert_eq!(task.task_status, expected);
}
