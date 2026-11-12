use super::*;
use crate::sandbox::store::{
    plan_agent_repeated_failure, validate_agent_repeated_failure, AgentRepeatedFailure,
    SandboxStoreError,
};
use crate::sandbox::{Agent, AgentLogEntry, TypedFailure};

fn ordinary(current: &Agent, at: &str, due: &str, class: &str) -> AgentWorkMutation {
    let mut next = current.clone();
    let fence = current.work.lease.as_ref().unwrap().fence;
    next.work.retry("worker", fence, due.into()).unwrap();
    next.revision += 1;
    next.updated_at = at.into();
    next.conversation.last_seq += 1;
    AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: "worker".into(),
        fence,
        agent: next,
        worktree: None,
        appended_at: at.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepFailed {
            step: current.work.step,
            failure: TypedFailure {
                class: class.into(),
                detail: "temporary failure".into(),
            },
        }],
    }
}

fn repeat(current: &Agent, marker: &AgentLogEntry) -> AgentRepeatedFailure {
    plan_agent_repeated_failure(
        current,
        Some(marker),
        "runtime_unavailable",
        "worker",
        current.work.lease.as_ref().unwrap().fence,
        MINUTE,
        TWO_MINUTES,
    )
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn repeated_failure_preserves_history_and_fences_every_metadata_change() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("repeat")).await.unwrap());
    let current = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    let first = ordinary(&current, NOW, MINUTE, "runtime_unavailable");
    store.commit_agent_work(first).await.unwrap();
    let current = store
        .claim_agent_work(&current.id, "worker", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    let marker = store
        .agent_log_entry(&current.id, current.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    let retry = repeat(&current, &marker);
    assert_eq!(
        retry.mutation.agent.conversation.last_seq,
        current.conversation.last_seq
    );
    assert_eq!(
        retry.mutation.agent.work.attempts,
        current.work.attempts + 1
    );
    assert_eq!(retry.mutation.agent.revision, current.revision + 1);
    assert!(retry.mutation.agent.work.lease.is_none());
    assert_eq!(retry.mutation.agent.work.receipts, current.work.receipts);
    assert_eq!(
        retry.mutation.agent.work.cleanup_debt,
        current.work.cleanup_debt
    );
    for variant in 0..10 {
        let mut invalid = retry.clone();
        match variant {
            0 => invalid.mutation.expected_agent_revision -= 1,
            1 => invalid.mutation.agent.revision += 1,
            2 => invalid.mutation.fence += 1,
            3 => invalid.mutation.agent.work.attempts += 1,
            4 => invalid.mutation.agent.conversation.last_seq += 1,
            5 => invalid.marker.seq -= 1,
            6 => invalid.mutation.agent_logs.push(marker.kind.clone()),
            7 => invalid.mutation.holder = "stale".into(),
            8 => {
                invalid.mutation.appended_at = TWO_MINUTES.into();
                invalid.mutation.agent.updated_at = TWO_MINUTES.into();
                invalid.mutation.agent.work.ready_at = "2026-09-05T00:03:00Z".into();
            }
            _ => {
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut invalid.marker.kind {
                    failure.class = "different".into();
                }
            }
        }
        assert_eq!(
            store
                .commit_agent_work_repeated_failure(invalid, None)
                .await,
            Err(SandboxStoreError::Conflict),
            "variant {variant}"
        );
        assert_eq!(
            store.get_agent(&current.id).await.unwrap(),
            Some(current.clone())
        );
    }
    // The old entry point cannot drop a diagnostic without explicit evidence.
    assert_eq!(
        store.commit_agent_work(retry.mutation.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    store
        .commit_agent_work_repeated_failure(retry.clone(), None)
        .await
        .unwrap();
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(retry.mutation.agent.clone())
    );
    assert_eq!(
        store
            .agent_log_entry(&current.id, current.conversation.last_seq)
            .await
            .unwrap(),
        Some(marker.clone())
    );
    assert_eq!(
        store.commit_agent_work_repeated_failure(retry, None).await,
        Err(SandboxStoreError::Conflict)
    );

    let next = store
        .claim_agent_work(
            &current.id,
            "worker",
            TWO_MINUTES.into(),
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let changed = ordinary(&next, TWO_MINUTES, "2026-09-05T00:03:00Z", "different");
    store.commit_agent_work(changed.clone()).await.unwrap();
    assert_eq!(
        changed.agent.conversation.last_seq,
        current.conversation.last_seq + 1
    );
}

#[tokio::test]
async fn repeated_failure_requires_same_step_class_and_exact_snapshot_tail() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("repeat-marker")).await.unwrap());
    let current = store
        .claim_agent_work(&creation.agent.id, "worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    store
        .commit_agent_work(ordinary(&current, NOW, MINUTE, "runtime_unavailable"))
        .await
        .unwrap();
    let current = store
        .claim_agent_work(&current.id, "worker", MINUTE.into(), TWO_MINUTES.into())
        .await
        .unwrap()
        .unwrap();
    let marker = store
        .agent_log_entry(&current.id, current.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    let retry = repeat(&current, &marker);
    for variant in 0..6 {
        let mut tail = marker.clone();
        match variant {
            0 => tail.agent_id = crate::AgentId("different".into()),
            1 => tail.seq -= 1,
            2 => {
                tail.kind = AgentLogKind::WorkStepCompleted {
                    step: current.work.step,
                }
            }
            3 => {
                if let AgentLogKind::WorkStepFailed { step, .. } = &mut tail.kind {
                    *step = AgentWorkStep::RemoveHome;
                }
            }
            4 => {
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut tail.kind {
                    failure.class = "different".into();
                }
            }
            _ => {
                if let AgentLogKind::WorkStepFailed { failure, .. } = &mut tail.kind {
                    failure.detail.push('!');
                }
            }
        }
        assert_eq!(
            validate_agent_repeated_failure(&current, &retry.mutation, &marker, Some(&tail)),
            Err(SandboxStoreError::Conflict)
        );
        if variant < 5 {
            assert!(plan_agent_repeated_failure(
                &current,
                Some(&tail),
                "runtime_unavailable",
                "worker",
                retry.mutation.fence,
                MINUTE,
                TWO_MINUTES
            )
            .unwrap()
            .is_none());
        }
    }
    assert_eq!(
        validate_agent_repeated_failure(&current, &retry.mutation, &marker, None),
        Err(SandboxStoreError::Conflict)
    );
    assert!(plan_agent_repeated_failure(
        &current,
        None,
        "runtime_unavailable",
        "worker",
        retry.mutation.fence,
        MINUTE,
        TWO_MINUTES
    )
    .unwrap()
    .is_none());
    for reserved in [
        crate::sandbox::store::AGENT_LOG_CAPACITY_FAILURE,
        crate::sandbox::store::AGENT_RUNTIME_LEDGER_HOLD,
    ] {
        let mut tail = marker.clone();
        if let AgentLogKind::WorkStepFailed { failure, .. } = &mut tail.kind {
            failure.class = reserved.into();
        }
        assert!(plan_agent_repeated_failure(
            &current,
            Some(&tail),
            reserved,
            "worker",
            retry.mutation.fence,
            MINUTE,
            TWO_MINUTES
        )
        .unwrap()
        .is_none());
    }
}
