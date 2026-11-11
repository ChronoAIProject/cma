use super::*;

fn failure(f: &Fixture) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: f.agent.id.clone(),
        seq: f.agent.conversation.last_seq,
        appended_at: f.agent.updated_at.clone(),
        kind: AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::PublishConnection,
            failure: f.agent.failure.clone().unwrap(),
        },
    }
}

#[test]
fn storage_proposal_fences_old_writer_without_consuming_a_log_slot() {
    // Physical byte/row validation belongs to S1/S4. Domain admission itself
    // needs zero extra slots even when the original head includes reserves.
    for head in [10_000, 10_001, 10_064, 10_128] {
        let mut f = Fixture::new();
        f.agent.conversation.last_seq = head;
        f.agent.work.lease = Some(AgentWorkLease {
            holder: "expired-worker".into(),
            fence: 7,
            expires_at: NOW.into(),
        });
        let log = failure(&f);
        let plan = f.context().plan(&log, NOW).unwrap();
        let next = plan.fenced_agent(&f.context(), NOW).unwrap();
        assert_eq!(plan.source(), &f.agent);
        assert_eq!(plan.original_failure(), &log);
        assert_eq!(plan.admitted_at(), NOW);
        assert_eq!(next.conversation.last_seq, head);
        assert_eq!(next.status, AgentStatus::Failed);
        let mut expected = f.agent.clone();
        expected.revision += 1;
        expected.work.next_fence += 1;
        expected.work.lease = None;
        expected.updated_at = NOW.into();
        assert_eq!(next, expected);
    }
}

#[test]
fn wrong_failure_source_never_gets_an_admission_proposal() {
    for mutation in 0..5 {
        let f = Fixture::new();
        let mut log = failure(&f);
        match mutation {
            0 => log.seq -= 1,
            1 => log.agent_id = crate::ids::AgentId("another-agent".into()),
            2 => log.appended_at = LATER.into(),
            3 => {
                log.kind = AgentLogKind::WorkStepFailed {
                    step: AgentWorkStep::CloseProcess,
                    failure: f.agent.failure.clone().unwrap(),
                }
            }
            _ => {
                log.kind = AgentLogKind::WorkStepFailed {
                    step: AgentWorkStep::PublishConnection,
                    failure: TypedFailure {
                        class: AGENT_LOG_CAPACITY_FAILURE.into(),
                        detail: "changed".into(),
                    },
                }
            }
        }
        assert!(matches!(
            f.context().plan(&log, NOW),
            Err(CapacityRecoveryDisposition::FailureEvidenceMismatch)
        ));
    }
}

#[test]
fn known_abort_retry_preserves_timestamp_and_every_input_fact() {
    for phase in 0..4 {
        let mut f = Fixture::new();
        let mut first =
            FirstMessageRecord::new(&f.agent.creation_key, "fixture input", NOW).unwrap();
        match phase {
            1 => first.fail_startup(NOW.into()).unwrap(),
            2 | 3 => {
                first.fence_dispatch_for(5, 6, 7, NOW.into()).unwrap();
                if phase == 3 {
                    first.mark_write_confirmed(NOW.into()).unwrap();
                }
                f.agent.conversation.active_turn = Some(5);
            }
            _ => {}
        }
        f.agent.conversation.first_message = Some(first.clone());
        let plan = f.context().plan(&failure(&f), NOW).unwrap();
        let first_proposal = plan.fenced_agent(&f.context(), NOW).unwrap();
        let retried = plan.fenced_agent(&f.context(), LATER).unwrap();
        assert_eq!(first_proposal, retried);
        assert_eq!(retried.conversation.first_message, Some(first));
        assert_eq!(retried.conversation, f.agent.conversation);
        assert_eq!(retried.work.receipts, f.agent.work.receipts);
        assert_eq!(retried.work.step, AgentWorkStep::None);
    }
}

#[test]
fn parent_intent_and_replaced_native_or_source_fence_stale_proposal() {
    for mutation in 0..8 {
        let mut f = Fixture::new();
        let plan = f.context().plan(&failure(&f), NOW).unwrap();
        match mutation {
            0 => f.sandbox.status = SandboxStatus::Retiring,
            1 => f.agent.requested_stop_reason = Some(AgentStopReason::Owner),
            2 => f.agent.revision += 1,
            3 => f.agent.conversation.last_seq += 1,
            4 => f.agent.work.next_fence += 1,
            5 => f.sandbox.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32)),
            6 => {
                f.sandbox
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .receipt
                    .as_mut()
                    .unwrap()
                    .storage_generation = "replaced".into()
            }
            _ => f.workspace.revision += 1,
        }
        assert!(plan.fenced_agent(&f.context(), LATER).is_err());
    }
}

#[test]
fn pending_original_work_never_gets_a_second_recovery_operation() {
    let mut f = Fixture::new();
    let original_failure = failure(&f);
    f.agent.status = AgentStatus::Ready;
    f.agent.failure = None;
    f.agent.work.step = AgentWorkStep::PublishConnection;
    assert!(matches!(
        f.context().plan(&original_failure, NOW),
        Err(CapacityRecoveryDisposition::ContinueOriginalWork)
    ));
}
