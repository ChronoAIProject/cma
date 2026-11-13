use super::*;
use layer2_core::sandbox::store::{AgentConnectionRecoveryCandidate, AgentStopRequest};
use layer3_adapter::wire::RequestFrame;

mod skills;
mod slow;

pub(super) const CASES: [(AgentWorkStep, WireOperation, AgentWorkStep); 10] = [
    (
        AgentWorkStep::AddWorktree,
        WireOperation::AddWorktree,
        AgentWorkStep::MaterializeHome,
    ),
    (
        AgentWorkStep::MaterializeHome,
        WireOperation::MaterializeAgentHome,
        AgentWorkStep::OpenConversation,
    ),
    (
        AgentWorkStep::OpenConversation,
        WireOperation::OpenAgent,
        AgentWorkStep::PublishReady,
    ),
    (
        AgentWorkStep::PublishReady,
        WireOperation::OpenAgent,
        AgentWorkStep::PublishConnection,
    ),
    (
        AgentWorkStep::DispatchFirstMessage,
        WireOperation::OpenAgent,
        AgentWorkStep::None,
    ),
    (
        AgentWorkStep::PublishConnection,
        WireOperation::OpenAgent,
        AgentWorkStep::None,
    ),
    (
        AgentWorkStep::CloseProcess,
        WireOperation::CloseAgent,
        AgentWorkStep::PublishStopped,
    ),
    (
        AgentWorkStep::EnsureProcessClosed,
        WireOperation::CloseAgent,
        AgentWorkStep::RemoveHome,
    ),
    (
        AgentWorkStep::RemoveHome,
        WireOperation::RemoveAgentHome,
        AgentWorkStep::RemoveWorktree,
    ),
    (
        AgentWorkStep::RemoveWorktree,
        WireOperation::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
    ),
];

pub(super) fn due(agent: &Agent) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&agent.work.ready_at).unwrap())
}

pub(super) fn worker<S: FixtureStore>(
    h: &Harness<S>,
    store: Arc<S>,
    holder: &str,
) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            // A restarted API must recover without a local staged attachment.
            registry: registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(store, registry),
        },
        AgentWorkerConfig::new(holder, DEPLOYMENT_ID),
    )
}

pub(super) async fn prepare<S: FixtureStore>(h: &Harness<S>, step: AgentWorkStep) -> Agent {
    let agent = h
        .admit_with_first_message("ledger-hold", vec![], Some("synthetic input"))
        .await;
    let startup = match step {
        AgentWorkStep::AddWorktree => Some(0),
        AgentWorkStep::MaterializeHome => Some(1),
        AgentWorkStep::OpenConversation => Some(2),
        AgentWorkStep::PublishReady => Some(3),
        AgentWorkStep::DispatchFirstMessage => Some(5),
        _ => None,
    };
    if let Some(count) = startup {
        for _ in 0..count {
            run_preparation(h, &agent.id).await;
        }
        return h.stored(&agent.id).await;
    }
    if step == AgentWorkStep::EnsureProcessClosed {
        for _ in 0..2 {
            run_preparation(h, &agent.id).await;
        }
        h.backend.lose_next_response(WireOperation::OpenAgent);
        run_preparation(h, &agent.id).await;
        let at = due(&h.stored(&agent.id).await);
        h.backend.fail_next(
            WireOperation::OpenAgent,
            "request_rejected",
            "synthetic rejection",
        );
        h.run(&agent.id, at).await;
        let failed = h.stored(&agent.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        h.store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&failed),
                requested_at: rfc3339_at(at),
            })
            .await
            .unwrap();
        return h.stored(&agent.id).await;
    }
    for _ in 0..6 {
        run_preparation(h, &agent.id).await;
    }
    let ready = h.stored(&agent.id).await;
    assert!(ready.conversation.active_turn.is_some());
    if step == AgentWorkStep::PublishConnection {
        h.store
            .schedule_agent_connection_recovery(
                AgentConnectionRecoveryCandidate::from_agent(&ready)
                    .unwrap()
                    .restart_request(rfc3339_at(SystemTime::now().max(h.now))),
            )
            .await
            .unwrap();
        return h.stored(&agent.id).await;
    }
    h.store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(SystemTime::now().max(h.now)),
        })
        .await
        .unwrap();
    run_preparation(h, &agent.id).await;
    if step == AgentWorkStep::CloseProcess {
        return h.stored(&agent.id).await;
    }
    for _ in 0..2 {
        run_preparation(h, &agent.id).await;
    }
    let stopped = h.stored(&agent.id).await;
    h.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(SystemTime::now().max(h.now)),
        })
        .await
        .unwrap();
    run_preparation(h, &agent.id).await;
    if step == AgentWorkStep::RemoveWorktree {
        run_preparation(h, &agent.id).await;
    }
    h.stored(&agent.id).await
}

async fn run_preparation<S: FixtureStore>(h: &Harness<S>, id: &AgentId) {
    let current = h.stored(id).await;
    let at = due(&current).max(h.now).max(SystemTime::now());
    assert!(h
        .worker
        .run_one(id, at)
        .await
        .unwrap_or_else(|error| { panic!("fixture failed at {:?}: {error}", current.work.step) }));
}

pub(super) fn refuse(h: &Harness<impl FixtureStore>, op: WireOperation) {
    h.backend.fail_next(
        op,
        "mutation_ledger_capacity",
        "runtime mutation ledger capacity is exhausted",
    );
}

pub(super) fn preserved(before: &Agent, after: &Agent) {
    assert_eq!(after.status, before.status);
    assert_eq!(after.work.step, before.work.step);
    assert!(after.work.lease.is_none());
    assert_eq!(after.work.receipts, before.work.receipts);
    assert_eq!(after.work.cleanup_debt, before.work.cleanup_debt);
    assert_eq!(after.work.resolved_skills, before.work.resolved_skills);
    assert_eq!(after.requested_stop_reason, before.requested_stop_reason);
    assert_eq!(after.failure, before.failure);
    assert_eq!(after.stopped_at, before.stopped_at);
    assert_eq!(after.deleted_at, before.deleted_at);
    let mut expected = before.conversation.clone();
    expected.last_seq = after.conversation.last_seq;
    assert_eq!(after.conversation, expected);
}

pub(super) fn request_identity(request: &RequestFrame) -> (u64, String, String) {
    macro_rules! identity {
        ($value:expr) => {
            (
                $value.authority_fence,
                $value.request.operation_id.clone(),
                $value.request.request_sha256.clone(),
            )
        };
    }
    match request {
        RequestFrame::AddWorktree(value) => identity!(value),
        RequestFrame::MaterializeAgentHome(value) => identity!(value),
        RequestFrame::OpenAgent(value) => identity!(value),
        RequestFrame::CloseAgent(value) => identity!(value),
        RequestFrame::RemoveAgentHome(value) => identity!(value),
        RequestFrame::RemoveWorktree(value) => identity!(value),
        _ => panic!("unexpected mutation fixture"),
    }
}

#[tokio::test(start_paused = true)]
async fn ledger_hold_paths_release_leases_keep_identity_and_advance_only_after_success() {
    for (step, op, next) in CASES {
        let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        h.now = SystemTime::now();
        let initial = prepare(&h, step).await;
        assert_eq!(initial.work.step, step);
        let parent = h.store.get_sandbox(&initial.sandbox_id).await.unwrap();
        let start = h.backend.requests().await.len();
        let mut previous = initial.clone();
        let mut at = due(&previous).max(h.now);
        for (attempt, delay) in [30, 60, 120, 240, 300, 300].into_iter().enumerate() {
            let worker = worker(&h, h.store.clone(), &format!("holder-{attempt}"));
            refuse(&h, op);
            assert!(worker.run_one(&initial.id, at).await.unwrap(), "{step:?}");
            let held = h.stored(&initial.id).await;
            // First startup attempt may record its original effect intent.
            if attempt > 0
                || !matches!(
                    step,
                    AgentWorkStep::AddWorktree
                        | AgentWorkStep::MaterializeHome
                        | AgentWorkStep::OpenConversation
                )
            {
                preserved(&previous, &held);
            }
            assert_eq!(held.status, initial.status);
            assert_eq!(held.work.step, step);
            assert_eq!(held.work.attempts, attempt as u32 + 1);
            assert_eq!(
                due(&held)
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    - layer2_core::seconds_from_rfc3339(&held.updated_at).unwrap(),
                delay
            );
            assert!(held.work.lease.is_none());
            assert!(!worker
                .run_one(&initial.id, due(&held) - Duration::from_secs(1))
                .await
                .unwrap());
            assert_eq!(
                h.store.get_sandbox(&initial.sandbox_id).await.unwrap(),
                parent
            );
            at = due(&held);
            previous = held;
        }
        let requests = h.backend.requests().await;
        let identities: Vec<_> = requests[start..]
            .iter()
            .filter(|(_, request)| request.operation() == op)
            .map(|(_, request)| request_identity(request))
            .collect();
        assert_eq!(identities.len(), 6, "{step:?}");
        for pair in identities.windows(2) {
            assert!(pair[0].0 < pair[1].0);
            assert_eq!((&pair[0].1, &pair[0].2), (&pair[1].1, &pair[1].2));
        }
        let logs = h
            .store
            .agent_log(&initial.id, initial.conversation.last_seq, 100)
            .await
            .unwrap();
        assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind,
            AgentLogKind::WorkStepFailed { failure, .. } if failure.class == "agent_runtime_ledger_hold")).count(), 1);
        assert!(worker(&h, h.store.clone(), "successful-holder")
            .run_one(&initial.id, at)
            .await
            .unwrap());
        let advanced = h.stored(&initial.id).await;
        assert_eq!(advanced.work.step, next, "{step:?}");
        assert_eq!(advanced.work.attempts, 0);
        assert!(advanced.work.lease.is_none());
    }
}
