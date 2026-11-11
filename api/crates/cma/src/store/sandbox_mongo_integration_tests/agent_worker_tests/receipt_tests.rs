use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_reopen_reconciliation_requires_every_exact_artifact() {
    let (client, name) = database("agent_work_reconcile").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "reconcile").await;
    let claimed = ready_publication_claim(claimed, "conversation-reconcile");
    replace_agent_fixture(&store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    let log_kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "conversation-reconcile".into(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.last_seq += u64::try_from(log_kinds.len()).unwrap();
    let event_kind = SandboxEventKind::AgentStatusChanged {
        agent_id: next.id.clone(),
        from: AgentStatus::Starting,
        to: AgentStatus::Ready,
    };
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: next.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: worktree.clone(),
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(event_kind.clone()),
            agent_logs: log_kinds.clone(),
        })
        .await
        .unwrap();
    let logs = log_kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| AgentLogEntry {
            agent_id: next.id.clone(),
            seq: offset as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect::<Vec<_>>();
    let receipt = PublicationReceipt {
        agent: next,
        worktree: Some(worktree),
        sign_in_sandbox: None,
        logs,
        event: Some(SandboxEvent {
            sandbox_id: parent.id.clone(),
            seq: 1,
            appended_at: NOW.into(),
            kind: event_kind,
        }),
    };
    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();

    assert!(reconcile_publication(&reopened, &receipt).await.unwrap());
    let mut wrong_agent = receipt.clone();
    wrong_agent.agent.title = Some("wrong".into());
    assert!(!reconcile_publication(&reopened, &wrong_agent)
        .await
        .unwrap());
    let mut wrong_worktree = receipt.clone();
    wrong_worktree.worktree.as_mut().unwrap().branch = "wrong".into();
    assert!(!reconcile_publication(&reopened, &wrong_worktree)
        .await
        .unwrap());
    let mut wrong_log = receipt.clone();
    wrong_log.logs[1].kind = AgentLogKind::Deleted;
    wrong_log.logs[1].appended_at = MINUTE.into();
    assert_eq!(
        reconcile_publication(&reopened, &wrong_log).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    let mut wrong_event = receipt.clone();
    wrong_event.event.as_mut().unwrap().kind = SandboxEventKind::Created;
    assert_eq!(
        reconcile_publication(&reopened, &wrong_event).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    reopened
        .agent_log
        .delete_one(doc! { "agent_id": receipt.agent.id.as_str(), "seq": 2_i64 })
        .await
        .unwrap();
    assert!(matches!(
        reconcile_publication(&reopened, &receipt).await,
        Err(SandboxStoreError::Corrupt(_))
    ));

    client.database(&name).drop().await.unwrap();
}
