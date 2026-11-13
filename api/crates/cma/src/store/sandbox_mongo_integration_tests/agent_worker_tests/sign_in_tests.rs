use super::super::*;
use crate::store::agent_worker_reconcile::reconcile_publication;
use crate::store::agent_worker_transaction::PublicationReceipt;
use layer2_core::sandbox::{AgentCleanupDebt, SandboxSignInState};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn startup_sign_in_notice_and_completion_are_atomic_with_parent_state() {
    let (client, name) = database("agent_startup_sign_in").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (mut parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "sign_in").await;
    let expected = parent.revision;
    parent.profile.content.model_access = ModelAccess::OwnCredentials;
    parent.credential.sign_in = SandboxSignInState::Complete;
    parent.revision += 1;
    store.save_sandbox(expected, parent.clone()).await.unwrap();
    let mut current = ready_publication_claim(claimed, "conversation-sign-in");
    current.work.step = AgentWorkStep::OpenConversation;
    current.work.resolved_skills = Some(vec![]);
    current
        .work
        .receipts
        .retain(|receipt| receipt.step != AgentWorkStep::OpenConversation);
    current.conversation.conversation_id = None;
    current.conversation.runtime_generation = None;
    current.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", current.id),
        request_sha256: "a".repeat(64),
    });
    replace_agent_fixture(&store, &current).await;
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    let notice = AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: "worker".into(),
        fence: current.work.lease.as_ref().unwrap().fence,
        agent: next.clone(),
        worktree: None,
        appended_at: NOW.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::SignInRequired {
            url: "https://login.example.test/device".into(),
            code: Some("fixture-code".into()),
            message: None,
        }],
    };
    let mut forged = notice.clone();
    forged.fence += 1;
    assert_eq!(
        store.commit_agent_work(forged).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    assert!(store
        .agent_log(&current.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    store.commit_agent_work(notice.clone()).await.unwrap();
    assert_eq!(
        store.commit_agent_work(notice).await,
        Err(SandboxStoreError::Conflict)
    );
    let waiting = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(waiting.credential.sign_in, SandboxSignInState::Required);
    assert_eq!(waiting.revision, parent.revision + 1);
    assert_eq!(store.agent_log(&current.id, 0, 10).await.unwrap().len(), 1);
    let mut pausing = client.start_session().await.unwrap();
    start_transaction(&mut pausing).await.unwrap();
    let pause_snapshot = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .session(&mut pausing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pause_snapshot.get_i64("revision").unwrap(),
        waiting.revision as i64
    );
    let previous_notice_seq = next.conversation.last_seq;
    let previous_revision = next.revision;
    next.revision += 1;
    next.conversation.last_seq += 1;
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: previous_revision,
            holder: "worker".into(),
            fence: next.work.lease.as_ref().unwrap().fence,
            agent: next.clone(),
            worktree: None,
            appended_at: NOW.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::SignInRequired {
                url: "https://login.example.test/device".into(),
                code: Some("fixture-code".into()),
                message: None,
            }],
        })
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(waiting.clone())
    );
    let repeated_receipt = PublicationReceipt {
        segmented: None,
        agent: next.clone(),
        worktree: None,
        logs: store
            .agent_log(&next.id, previous_notice_seq, 10)
            .await
            .unwrap(),
        event: None,
        sign_in_sandbox: Some(waiting.clone()),
    };
    assert!(reconcile_publication(&store, &repeated_receipt)
        .await
        .unwrap());
    let mut pause_candidate = waiting.clone();
    pause_candidate.begin_pause(NOW.into(), None).unwrap();
    let pause_error = store
        .sandboxes
        .update_one(
            doc! { "_id": parent.id.as_str(), "revision": waiting.revision as i64 },
            doc! { "$set": crate::store::sandboxes::sandbox_replacement_fields(&pause_candidate).unwrap() },
        )
        .session(&mut pausing)
        .await
        .expect_err("even a repeated challenge invalidates an older owner-pause snapshot");
    assert!(is_transient(&pause_error));
    let _ = pausing.abort_transaction().await;
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(waiting.clone())
    );
    drop(store);

    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(next.clone())
    );
    let mut opened = ready_publication_claim(next, "conversation-sign-in");
    opened.work.cleanup_debt.clear();
    replace_agent_fixture(&store, &opened).await;
    let fence = opened.work.lease.as_ref().unwrap().fence;
    let mut ready = opened.clone();
    ready
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    ready
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    ready.conversation.last_seq += 2;
    let worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    let mutation = AgentWorkMutation {
        expected_agent_revision: opened.revision,
        holder: "worker".into(),
        fence,
        agent: ready.clone(),
        worktree: Some(VersionedWorktree {
            expected_revision: worktree_revision,
            worktree: worktree.clone(),
        }),
        appended_at: NOW.into(),
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: ready.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }),
        agent_logs: vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-sign-in".into(),
                runtime_generation: 1,
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            },
        ],
    };
    store.commit_agent_work(mutation).await.unwrap();
    let completed = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(completed.credential.sign_in, SandboxSignInState::Complete);
    assert_eq!(completed.revision, waiting.revision + 1);
    let logs = store
        .agent_log(&ready.id, opened.conversation.last_seq, 10)
        .await
        .unwrap();
    let event = store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let receipt = PublicationReceipt {
        segmented: None,
        agent: ready,
        worktree: Some(worktree),
        logs,
        event: Some(event),
        sign_in_sandbox: Some(completed.clone()),
    };
    assert!(reconcile_publication(&store, &receipt).await.unwrap());
    let mut wrong = receipt.clone();
    wrong.sign_in_sandbox.as_mut().unwrap().credential.sign_in = SandboxSignInState::Required;
    assert!(!reconcile_publication(&store, &wrong).await.unwrap());
    client.database(&name).drop().await.unwrap();
}
