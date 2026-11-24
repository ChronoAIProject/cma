use super::*;
use crate::agent_manager::relay::tests::{acquire, cluster, Instance};
use crate::sandbox::worker_tests::owned_reopen_tests::workers_with_renewal;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore, SandboxWorkerStore};

mod dispatch;

#[tokio::test]
async fn issue_1164_old_revive_admission_does_not_expire_a_fresh_reopen_phase() {
    for with_conversation in [true, false] {
        let (fixture, mut reopening, mut now) = if with_conversation {
            let (fixture, reopening, _, now) = retained_reopen_tests::restored_reopen().await;
            (fixture, reopening, now)
        } else {
            let fixture = Fixture::native().await;
            let created = fixture.harness.create().await;
            let mut now = start_time(&created);
            let active = fixture
                .drive(
                    &created.id,
                    &mut now,
                    SandboxStatus::Active,
                    &runtime_image(),
                )
                .await;
            let paused = fixture.pause(&active, &mut now).await;
            fixture
                .harness
                .store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                    paused.id.clone(),
                    paused.owner,
                    rfc3339_at(now),
                    SandboxReviveCause::ExplicitRequest,
                ))
                .await
                .unwrap();
            let mut reopening = fixture.harness.stored(&active.id).await;
            for _ in 0..10 {
                if matches!(
                    reopening.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Revive {
                        phase: SandboxRevivePhase::ConversationReopen,
                        ..
                    })
                ) {
                    break;
                }
                now += Duration::from_secs(6);
                fixture.step(&active.id, now, &runtime_image()).await;
                reopening = fixture.harness.stored(&active.id).await;
            }
            (fixture, reopening, now)
        };
        let Some(SandboxLifecycleOperation::Revive {
            authority,
            phase: SandboxRevivePhase::ConversationReopen,
            ..
        }) = reopening.lifecycle.operation.as_mut()
        else {
            panic!("fixture did not enter ConversationReopen")
        };
        authority.requested_at = rfc3339_at(SystemTime::now() - Duration::from_secs(1801));
        let previous = reopening.revision;
        reopening.revision += 1;
        fixture
            .harness
            .store
            .save_sandbox(previous, reopening.clone())
            .await
            .unwrap();
        let active = fixture
            .drive(
                &reopening.id,
                &mut now,
                SandboxStatus::Active,
                &runtime_image(),
            )
            .await;
        assert_eq!(active.lifecycle.home, reopening.lifecycle.home);
    }
}

#[tokio::test]
async fn issue_1164_reopen_retries_keep_the_first_attempts_deadline() {
    let (fixture, mut reopening, _, mut now) = retained_reopen_tests::restored_reopen().await;
    let wall = SystemTime::now();
    let Some(SandboxLifecycleOperation::Revive { authority, .. }) =
        reopening.lifecycle.operation.as_mut()
    else {
        panic!("fixture is missing its revive operation");
    };
    authority.requested_at = rfc3339_at(wall - Duration::from_secs(1900));
    let previous = reopening.revision;
    reopening.revision += 1;
    reopening.work.attempts = 2;
    let store = &fixture.harness.store;
    store
        .save_sandbox(previous, reopening.clone())
        .await
        .unwrap();
    let failed = || SandboxEventKind::WorkStepFailed {
        step: SandboxWorkStep::RunLifecycleOperation,
        failure: layer2_core::sandbox::TypedFailure {
            class: "retryable_transient".into(),
            detail: "synthetic reopen interruption".into(),
        },
    };
    for (seconds_ago, event) in [
        (1850, failed()),
        (
            1820,
            SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            },
        ),
        (1801, failed()),
        (1, failed()),
    ] {
        store
            .append_sandbox_event(
                &reopening.id,
                rfc3339_at(wall - Duration::from_secs(seconds_ago)),
                event,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .lifecycle_retry_started_at(
                &reopening.id,
                &rfc3339_at(wall - Duration::from_secs(1900)),
            )
            .await
            .unwrap(),
        Some(rfc3339_at(wall - Duration::from_secs(1801))),
    );
    fixture.backend.reject_reopen.store(true, Ordering::SeqCst);
    now += Duration::from_secs(6);
    fixture.step(&reopening.id, now, &runtime_image()).await;
    let expired = fixture.harness.stored(&reopening.id).await;
    assert_eq!(expired.status, SandboxStatus::Failed);
    assert_eq!(expired.lifecycle.home, reopening.lifecycle.home);
    assert_eq!(
        expired.failure.unwrap().detail,
        "retained conversation reopen deadline elapsed",
    );
}

async fn ready_on_wall_clock(fixture: &Fixture, mut reopening: Sandbox) -> (Sandbox, SystemTime) {
    // The lifecycle fixture advances a logical worker clock in six-second
    // steps. Start this claim on wall time so a 15-second lease is really short.
    let now = SystemTime::now();
    let before = reopening.revision;
    reopening.work.ready_at = rfc3339_at(now);
    reopening.revision += 1;
    fixture
        .harness
        .store
        .save_sandbox(before, reopening.clone())
        .await
        .unwrap();
    (reopening, now)
}

async fn maintain_ownership(instances: &[Instance]) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        for instance in instances {
            instance.coordinator.maintain().await.unwrap();
        }
    }
}

#[tokio::test]
async fn issue_1164_idle_pause_failed_revive_and_public_retire() {
    for _ in 0..2 {
        let (fixture, reopening, agent, mut now) =
            retained_reopen_tests::restored_reopen_after_pause(true).await;
        let app = router(&fixture);
        fixture.backend.reject_reopen.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        now += Duration::from_secs(6);
        fixture.step(&reopening.id, now, &runtime_image()).await;
        let failed = fixture.harness.stored(&reopening.id).await;
        let result = public_call(&app, &failed.id, Method::GET, "owner", StatusCode::OK).await;
        assert_eq!(failed.status, SandboxStatus::Failed);
        assert_eq!(failed.lifecycle.home, reopening.lifecycle.home);
        assert_eq!(result["lifecycle"]["result"]["code"], "lifecycle_failed");
        assert_eq!(
            result["lifecycle"]["result"]["actions"],
            serde_json::json!(["observe", "retire"])
        );
        eprintln!(
            "ConversationReopen -> Failed (lifecycle_failed) logical=6s wall={}ms",
            started.elapsed().as_millis()
        );
        let started = std::time::Instant::now();
        public_call(
            &app,
            &failed.id,
            Method::DELETE,
            "other",
            StatusCode::NOT_FOUND,
        )
        .await;
        let receipt = tokio::time::timeout(
            Duration::from_secs(3),
            public_call(
                &app,
                &failed.id,
                Method::DELETE,
                "owner",
                StatusCode::ACCEPTED,
            ),
        )
        .await
        .unwrap();
        assert_eq!(receipt["status"], "retiring");
        eprintln!(
            "owner DELETE 202 Retiring wall={}ms",
            started.elapsed().as_millis()
        );
        retire_and_prove(&fixture, &failed, &agent, now).await;
        let retired = fixture.harness.stored(&failed.id).await;
        eprintln!("Retired, Agent Stopped, claim and runtime inventory empty, one effective delete, wall={}ms", started.elapsed().as_millis());
        assert_eq!(retired.status, SandboxStatus::Retired);
    }
}

#[tokio::test]
async fn issue_1164_peer_and_local_reopen_follow_renewals_past_old_deadlines() {
    let attempt = async |remote| {
        let (fixture, reopening, agent, _) = retained_reopen_tests::restored_reopen().await;
        let (reopening, now) = ready_on_wall_clock(&fixture, reopening).await;
        let (ownership, replicas) = cluster(2).await;
        let (owner, _authority) = acquire(&ownership, &replicas, &agent.id).await;
        let workers = workers_with_renewal(&fixture.harness, &replicas, true);
        let caller = workers[if remote {
            (owner + 1) % replicas.len()
        } else {
            owner
        }]
        .0
        .clone();
        fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
        let id = reopening.id.clone();
        let started = std::time::Instant::now();
        let mut running = tokio::spawn(async move { caller.run_one(&id, now).await });
        let verifying = async {
            tokio::time::timeout(
                Duration::from_secs(3),
                fixture.backend.reopen_entered.notified(),
            )
            .await
            .unwrap();
            let first = fixture
                .harness
                .stored(&reopening.id)
                .await
                .work
                .lease
                .unwrap();
            let early = tokio::time::timeout(Duration::from_secs(62), &mut running).await;
            fixture.backend.reopen_release.notify_one();
            assert!(
                early.is_err(),
                "remote={remote} reopen ended after {}ms: {early:?}",
                started.elapsed().as_millis()
            );
            let renewed = fixture
                .harness
                .stored(&reopening.id)
                .await
                .work
                .lease
                .unwrap();
            assert_eq!(renewed.holder, first.holder);
            assert_eq!(renewed.fence, first.fence);
            assert!(renewed.expires_at > first.expires_at);
            assert!(tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap()
                .unwrap());
        };
        tokio::select! {
            _ = verifying => (),
            _ = maintain_ownership(&replicas) => unreachable!(),
        }
        let resumed = fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap();
        let current = fixture.harness.stored(&reopening.id).await;
        assert_eq!(resumed.status, AgentStatus::Ready);
        assert_eq!(resumed.worktree_id, agent.worktree_id);
        assert_eq!(
            resumed.conversation.conversation_id,
            agent.conversation.conversation_id
        );
        assert_eq!(current.lifecycle.home, reopening.lifecycle.home);
        let mut finish = SystemTime::now().max(stamp_time(&current.work.ready_at));
        let active = fixture
            .drive(
                &reopening.id,
                &mut finish,
                SandboxStatus::Active,
                &runtime_image(),
            )
            .await;
        assert_eq!(active.lifecycle.home, reopening.lifecycle.home);
        eprintln!("remote={remote} ConversationReopen -> Active, same Agent, conversation and home, wall={}ms", started.elapsed().as_millis());
    };
    tokio::join!(attempt(true), attempt(false));
}

#[tokio::test]
async fn issue_1164_owner_delete_fences_a_held_peer_reopen() {
    let (fixture, reopening, agent, _) = retained_reopen_tests::restored_reopen().await;
    let (reopening, now) = ready_on_wall_clock(&fixture, reopening).await;
    let (ownership, replicas) = cluster(2).await;
    let (owner, _) = acquire(&ownership, &replicas, &agent.id).await;
    let workers = workers_with_renewal(&fixture.harness, &replicas, true);
    let caller = workers[(owner + 1) % replicas.len()].0.clone();
    fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
    let id = reopening.id.clone();
    let running = tokio::spawn(async move { caller.run_one(&id, now).await });
    tokio::time::timeout(
        Duration::from_secs(3),
        fixture.backend.reopen_entered.notified(),
    )
    .await
    .unwrap();
    let app = router(&fixture);
    let reply = tokio::time::timeout(
        Duration::from_secs(3),
        public_call(
            &app,
            &reopening.id,
            Method::DELETE,
            "owner",
            StatusCode::ACCEPTED,
        ),
    )
    .await
    .unwrap();
    assert_eq!(reply["status"], "retiring");
    fixture.backend.reopen_release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap(),
        Err(SandboxStoreError::Conflict),
    );
    assert_eq!(
        fixture
            .harness
            .store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    retire_and_prove(&fixture, &reopening, &agent, now).await;
}

#[tokio::test]
async fn issue_1164_peer_reopen_stops_when_the_original_claim_expires() {
    let (fixture, reopening, agent, _) = retained_reopen_tests::restored_reopen().await;
    let (reopening, now) = ready_on_wall_clock(&fixture, reopening).await;
    let (ownership, replicas) = cluster(2).await;
    let (owner, _) = acquire(&ownership, &replicas, &agent.id).await;
    let workers = workers_with_renewal(&fixture.harness, &replicas, true);
    let claim = fixture
        .harness
        .store
        .claim_sandbox_work(
            &reopening.id,
            "unrenewed",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(3)),
        )
        .await
        .unwrap()
        .unwrap();
    fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
    let relay = replicas[(owner + 1) % replicas.len()].relay.clone();
    let agent_id = agent.id.clone();
    let attempted = claim.clone();
    let caller = relay.clone();
    let call = tokio::spawn(async move { caller.reopen_agent(&attempted, &agent_id, false).await });
    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.backend.reopen_entered.notified(),
    )
    .await
    .unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    // A later invocation with that same expired claim must never open a new
    // channel, even though the Agent's owner and the Sandbox revision match.
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        relay.reopen_agent(&claim, &agent.id, false)
    )
    .await
    .unwrap()
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        fixture.backend.reopen_entered.notified(),
    )
    .await
    .is_err());
    fixture.backend.reopen_release.notify_one();
    assert!(!workers[owner]
        .1
        .target_is_attached(&crate::sandbox::live_agents::LiveAgentTarget {
            agent_id: agent.id,
            conversation_id: agent.conversation.conversation_id.unwrap(),
            runtime_generation: reopening.runtime.unwrap().provision_generation.unwrap(),
        }));
}
