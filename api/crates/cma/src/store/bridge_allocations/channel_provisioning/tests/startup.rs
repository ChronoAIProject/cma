use super::*;
use layer2_core::{
    bridge::allocation::AllocationState,
    sandbox::{
        store::{AgentStore, AgentWorkerStore, WorktreeStore},
        AgentStatus, AgentWorkStep, SandboxStatus, WorktreeStatus,
    },
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    wire::RequestFrame,
};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_real_open_commits_ready_and_ordinary_worker_reconnects_same_conversation_with_greater_fence(
) {
    let mut f = Fixture::new().await;
    let allocation = f.ready().await;
    assert_eq!(allocation.state, AllocationState::Ready);
    assert!(allocation.receipt.is_some() && allocation.lease.is_none());
    let target = allocation.placement.target();
    let agent = f
        .resources
        .get_agent(&target.agent_id)
        .await
        .unwrap()
        .unwrap();
    let tree = f
        .resources
        .get_worktree(&target.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let sandbox = f
        .resources
        .get_sandbox(&target.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.status, AgentStatus::Ready);
    assert_eq!(tree.status, WorktreeStatus::Ready);
    assert_eq!(sandbox.status, SandboxStatus::Active);
    assert_eq!(agent.work.step, AgentWorkStep::PublishConnection);
    assert!(agent.work.next_fence > allocation.lease_fence);
    assert_eq!(agent.work.receipts.len(), 3);
    assert!(agent.work.cleanup_debt.is_empty());
    assert_eq!(agent.work.resolved_skills, Some(Vec::new()));
    let opened = agent.conversation.conversation_id.clone().unwrap();
    assert_eq!(agent.conversation.runtime_generation, Some(1));
    let rows = f
        .client
        .database(&f.database)
        .collection::<mongodb::bson::Document>("bridge_channel_capabilities");
    assert_eq!(
        rows.count_documents(doc! {"record.opened_conversation":&opened,"revoked":false})
            .await
            .unwrap(),
        1
    );
    f.reopen_driver();
    let registry = crate::sandbox::channel_startup::test_support::publish_connection(
        f.resources.clone(),
        f.adapter.clone(),
        &agent.id,
    )
    .await;
    let after = f.resources.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(after.status, AgentStatus::Ready);
    assert_eq!(after.work.step, AgentWorkStep::None);
    assert_eq!(
        after.conversation.conversation_id.as_deref(),
        Some(opened.as_str())
    );
    assert!(after.work.next_fence > agent.work.next_fence);
    assert!(f
        .resources
        .due_agent_work("2999-01-01T00:00:00Z", 10)
        .await
        .unwrap()
        .is_empty());
    let handle = RuntimeHandle::new(&sandbox.runtime.unwrap().runtime_id);
    let runtime = f.backend.snapshot(&handle).await.unwrap();
    assert_eq!(runtime.workspaces.len(), 1);
    assert_eq!(runtime.worktrees.len(), 1);
    assert_eq!(runtime.homes.len(), 1);
    assert_eq!(runtime.agents.len(), 1);
    let frames = f
        .backend
        .requests()
        .await
        .into_iter()
        .filter_map(|(_, frame)| {
            if let RequestFrame::OpenAgent(frame) = frame {
                Some(frame)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].request, frames[1].request);
    assert!(frames[1].authority_fence > frames[0].authority_fence);
    assert!(!format!("{runtime:?}").contains(HUMAN_SECRET));
    assert!(!format!("{runtime:?}").contains(RUNTIME_SECRET));
    drop(registry);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_lost_open_reply_replays_exact_intent_after_restart_and_creates_one_conversation() {
    let mut f = Fixture::new().await;
    let snapshot = f.agent_start().await;
    f.backend.lose_next_response(WireOperation::OpenAgent);
    assert!(f
        .startup_driver()
        .advance(&f.authority, &f.lease, &snapshot)
        .await
        .is_err());
    let target = snapshot.allocation.placement.target();
    let held = f
        .resources
        .get_agent(&target.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.status, AgentStatus::Starting);
    assert_eq!(held.work.step, AgentWorkStep::None);
    assert!(held.conversation.conversation_id.is_none());
    assert_eq!(held.work.cleanup_debt.len(), 3);
    assert!(held.work.receipts.is_empty());
    f.release_current_claim().await;
    f.reopen_driver();
    let allocation = f.ready().await;
    assert_eq!(allocation.state, AllocationState::Ready);
    let requests = f.backend.requests().await;
    let opens = requests
        .into_iter()
        .filter_map(|(_, frame)| {
            if let RequestFrame::OpenAgent(frame) = frame {
                Some(frame)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[0].request, opens[1].request);
    assert!(opens[1].authority_fence > opens[0].authority_fence);
    let runtime = f
        .backend
        .snapshot(&RuntimeHandle::new(
            &snapshot.sandbox.runtime.unwrap().runtime_id,
        ))
        .await
        .unwrap();
    assert_eq!(runtime.agents.len(), 1);
    assert_eq!(runtime.homes.len(), 1);
    assert_eq!(runtime.worktrees.len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_real_open_without_durable_tool_confirmation_cannot_publish_ready() {
    let mut f = Fixture::new().await;
    let snapshot = f.agent_start().await;
    let mut driver = f.startup_driver();
    driver.adapter = Arc::new(CodexAdapter::with_backend(
        f.backend.clone(),
        CodexSandboxSpec::default(),
    ));
    assert!(matches!(
        driver.advance(&f.authority, &f.lease, &snapshot).await,
        Err(AllocationError::AuthorityUnavailable)
    ));
    let held = f
        .resources
        .get_agent(&snapshot.allocation.placement.target().agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.status, AgentStatus::Starting);
    assert!(held.conversation.conversation_id.is_none());
    assert!(f
        .allocations
        .channel_allocation(&snapshot.allocation.id)
        .await
        .unwrap()
        .unwrap()
        .receipt
        .is_none());
    let ready = f
        .startup_driver()
        .advance(&f.authority, &f.lease, &snapshot)
        .await
        .unwrap();
    assert_eq!(ready.state, AllocationState::Ready);
    f.close().await;
}
