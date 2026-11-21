use super::*;
use crate::sandbox::store::{
    plan_agent_cleanup_capacity_retry, AgentControlStore, AgentControlTarget, AgentStopRequest,
    SandboxStoreError,
};
use crate::sandbox::AgentStopReason;

#[tokio::test]
async fn cleanup_capacity_retry_requires_retained_evidence_in_memory_snapshot() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("capacity-retry"))
            .await
            .unwrap(),
    );
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&creation.agent),
            reason: AgentStopReason::Owner,
            requested_at: NOW.into(),
        })
        .await
        .unwrap();
    let current = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let fence = current.work.lease.as_ref().unwrap().fence;
    let first =
        plan_agent_cleanup_capacity_retry(&current, None, "worker", fence, NOW, MINUTE).unwrap();
    let original = store.agent_log(&current.id, 0, 200).await.unwrap();
    let mut unproved = first.clone();
    unproved.agent_logs.clear();
    unproved.agent.conversation.last_seq -= 1;
    assert_eq!(
        store.commit_agent_work(unproved).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    store.commit_agent_work(first).await.unwrap();
    let current = store
        .claim_agent_work(&current.id, "worker", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    let last = store
        .agent_log(&current.id, current.conversation.last_seq - 1, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let fence = current.work.lease.as_ref().unwrap().fence;
    let duplicate =
        plan_agent_cleanup_capacity_retry(&current, None, "worker", fence, MINUTE, TWO_MINUTES)
            .unwrap();
    assert_eq!(
        store.commit_agent_work(duplicate).await,
        Err(SandboxStoreError::Conflict)
    );
    let retry = plan_agent_cleanup_capacity_retry(
        &current,
        Some(&last),
        "worker",
        fence,
        MINUTE,
        TWO_MINUTES,
    )
    .unwrap();
    assert!(retry.agent_logs.is_empty());
    let mut expired = retry.clone();
    expired.appended_at = TWO_MINUTES.into();
    expired.agent.updated_at = TWO_MINUTES.into();
    expired.agent.work.ready_at = "2026-09-05T00:03:00Z".into();
    assert_eq!(
        store.commit_agent_work(expired).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    store.commit_agent_work(retry.clone()).await.unwrap();
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(retry.agent)
    );
    let retained = store.agent_log(&current.id, 0, 200).await.unwrap();
    assert_eq!(retained.len(), original.len() + 1);
    assert_eq!(&retained[..original.len()], original.as_slice());
    assert_eq!(retained.last(), Some(&last));
}
