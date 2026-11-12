use super::*;

#[tokio::test]
async fn startup_is_isolated_receipted_published_and_records_runtime_updates() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let first = harness.admit("first", Vec::new()).await;
    let second = harness.admit("second", Vec::new()).await;
    let first = harness.drive_ready(&first.id).await;
    let second = harness.drive_ready(&second.id).await;
    let first_worktree = harness
        .store
        .get_worktree(&first.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let second_worktree = harness
        .store
        .get_worktree(&second.worktree_id)
        .await
        .unwrap()
        .unwrap();

    assert_ne!(first.id, second.id);
    assert_ne!(first_worktree.name, second_worktree.name);
    assert_ne!(first_worktree.branch, second_worktree.branch);
    assert_ne!(first_worktree.path, second_worktree.path);
    assert_ne!(
        first.conversation.conversation_id,
        second.conversation.conversation_id
    );
    assert_eq!(first_worktree.status, WorktreeStatus::Ready);
    assert_eq!(second_worktree.status, WorktreeStatus::Ready);
    assert_eq!(
        first
            .work
            .receipts
            .iter()
            .map(|receipt| receipt.step)
            .collect::<Vec<_>>(),
        [
            AgentWorkStep::AddWorktree,
            AgentWorkStep::MaterializeHome,
            AgentWorkStep::OpenConversation,
        ]
    );
    assert_eq!(
        first.work.receipts[0].operation_id,
        format!("worktree:project:{}:add:v1", first_worktree.name)
    );
    assert_eq!(
        first.work.receipts[1].operation_id,
        format!("agent:{}:home:create:v1", first.id)
    );
    assert_eq!(
        first.work.receipts[2].operation_id,
        format!("agent:{}:open:v1", first.id)
    );
    assert!(first
        .work
        .receipts
        .iter()
        .all(|receipt| receipt.request_sha256.len() == 64));

    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 2);
    assert_eq!(snapshot.homes.len(), 2);
    assert_eq!(snapshot.agents.len(), 2);
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(first.id.to_string())));
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(second.id.to_string())));

    let logs = harness.store.agent_log(&first.id, 0, 20).await.unwrap();
    assert!(matches!(
        logs.as_slice(),
        [
            _,
            _,
            _,
            _,
            layer2_core::sandbox::AgentLogEntry {
                kind: AgentLogKind::ConversationOpened { .. },
                ..
            },
            layer2_core::sandbox::AgentLogEntry {
                kind: AgentLogKind::StatusChanged {
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                },
                ..
            },
            layer2_core::sandbox::AgentLogEntry {
                kind: AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                },
                ..
            }
        ]
    ));
    let events = harness
        .store
        .sandbox_events(&first.sandbox_id, 0, 40)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        layer2_core::sandbox::SandboxEventKind::AgentStatusChanged {
            agent_id,
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        } if agent_id == &first.id
    )));

    let target = Harness::target(&first);
    let connection = harness.registry.resolve(&target).expect("published agent");
    assert_eq!(connection.capabilities, first.capabilities);
    assert!(harness
        .registry
        .resolve(&LiveAgentTarget {
            runtime_generation: 2,
            ..target.clone()
        })
        .is_none());
    assert!(harness
        .registry
        .resolve(&LiveAgentTarget {
            conversation_id: "another-conversation".into(),
            ..target
        })
        .is_none());

    harness
        .backend
        .send_agent_update(
            &harness.runtime,
            &AgentSlot::new(first.id.to_string()),
            ConversationUpdate::Lost { frames: 2 },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let logs = harness.store.agent_log(&first.id, 0, 20).await.unwrap();
            if logs
                .iter()
                .any(|entry| matches!(entry.kind, AgentLogKind::ConversationGap { frames: 2 }))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("runtime update persisted");
}

// These first-message fixtures reuse one wall-clock instant for startup steps.
// Freeze elapsed time too, so a step cannot publish the next second while the
// following claim still uses the previous one. Retry deadlines remain explicit.
