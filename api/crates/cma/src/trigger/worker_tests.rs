use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    seconds_from_rfc3339, ActivityQuery, ActivityRecord, ActivityStore, AuthorityProvenance,
    BillingMode, CommercialPolicy, CommitAttributionPolicy, ConnectedAccount, ConnectionSnapshot,
    ConnectionState, ContributionPolicy, CredentialRoute, DelegationState, EditableField,
    ExecutorPolicy, InputFieldDefinition, InputFieldKind, Launch, LaunchAuthority, LaunchQuery,
    LaunchReviewAdmission, LaunchState, LaunchStore, OverridePolicy, PermissionMode,
    PricingSnapshot, PublicProjectionPolicy, PublicTriggerId, PublishedRevision,
    PublisherAuthority, PullRequestDefaults, QuotaSnapshot, RepositoryAccessPolicy,
    RepositoryIdentity, RepositoryOperationClass, RepositoryVisibility, RequiredCapability,
    ResourcePolicy, RuntimeAdmissionSnapshot, ScopeClass, SessionSize, SetupPolicy, SourceMode,
    StartAdmissionSnapshot, StoreError, SubjectId, TriggerAuthorityVerifier, TriggerDraft,
    TriggerId, TriggerPage, TriggerPageRequest, TriggerRevision, TriggerRevisionContent,
    TriggerRevisionId, TriggerRolloutPolicy, TriggerService, TriggerServiceError,
    TriggerSessionPolicy, TriggerStore, TriggerStoreError, TriggerTask, UpstreamRevision,
    WorkerLease, WorkflowChangePolicy, TRIGGER_SCHEMA_VERSION,
};
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use super::*;

const NOW: &str = "2026-08-27T00:00:00Z";
const LATER: &str = "2026-08-27T00:00:01Z";

#[derive(Default)]
pub(crate) struct RecordingActivityStore {
    pub(crate) records: AsyncMutex<Vec<ActivityRecord>>,
}

#[async_trait]
impl ActivityStore for RecordingActivityStore {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().await.push(record.clone());
        Ok(())
    }

    async fn recent(&self, _query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        Ok(self.records.lock().await.clone())
    }
}

struct TestClock {
    times: Mutex<VecDeque<SystemTime>>,
}

impl WorkerClock for TestClock {
    fn now(&self) -> SystemTime {
        self.times
            .lock()
            .unwrap()
            .pop_front()
            .expect("the worker sampled only the declared test times")
    }
}

fn test_worker<T: EffectExecutor + 'static>(
    service: Arc<TriggerService>,
    executor: Arc<T>,
    holder: &str,
    times: &[&str],
) -> EffectWorker {
    let clock = Arc::new(TestClock {
        times: Mutex::new(
            times
                .iter()
                .map(|value| {
                    SystemTime::UNIX_EPOCH
                        + Duration::from_secs(seconds_from_rfc3339(value).unwrap())
                })
                .collect(),
        ),
    });
    EffectWorker::with_clock(service, executor, holder, clock).unwrap()
}

#[tokio::test]
async fn claim_is_committed_before_the_executor_and_success_releases_the_lease() {
    let fixture = Fixture::new();
    let executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&fixture.store),
        fixture.launch.authority.clone().unwrap(),
    ));
    let worker = test_worker(
        fixture.service(),
        executor.clone(),
        "worker-a",
        &[NOW, LATER],
    );

    let outcome = worker
        .run_once(&fixture.launch.id, 0, "run-one")
        .await
        .unwrap();
    assert_eq!(outcome, EffectRunOutcome::Completed);
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);
    assert!(executor.saw_committed_claim.load(Ordering::SeqCst) > 0);

    let stored = fixture.store.current();
    assert_eq!(stored.state, LaunchState::ForkPending);
    assert_eq!(stored.revision_number, 4);
    assert!(stored.lease.is_none());
    assert_eq!(stored.effects.len(), 1);
    assert_eq!(stored.effects[0].state, layer2_core::EffectState::Succeeded);
}

#[tokio::test]
async fn observed_session_wait_is_durable_without_a_failure_or_budget_spend() {
    let mut launch = Fixture::new().launch;
    launch.state = LaunchState::SandboxPending;
    launch.effects.clear();
    launch.lease = None;
    let store = Arc::new(TestStore::new(launch.clone()));
    let service = Arc::new(TriggerService::new(
        store.clone(),
        store.clone(),
        Arc::new(NeverAuthority),
    ));
    let worker = test_worker(
        service,
        Arc::new(WaitingExecutor),
        "session-wait-worker",
        &[NOW, LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "session-wait")
            .await
            .unwrap(),
        EffectRunOutcome::Deferred
    );
    let retained = store.current();
    let effect = retained.effects.last().unwrap();
    assert_eq!(effect.kind, layer2_core::LaunchEffectKind::Sandbox);
    assert_eq!(effect.state, layer2_core::EffectState::Reconciling);
    assert_eq!(effect.attempt, 1);
    assert_eq!(effect.reconciliation_attempt, 0);
    assert!(effect.last_failure.is_none());
    assert!(retained.failure.is_none());
    assert_eq!(retained.state, LaunchState::SandboxPending);
    assert!(retained.lease.is_none());
}

#[tokio::test]
async fn effect_activity_retains_only_closed_safe_identifiers() {
    let fixture = Fixture::new();
    let executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&fixture.store),
        fixture.launch.authority.clone().unwrap(),
    ));
    let activity_store = Arc::new(RecordingActivityStore::default());
    let worker = test_worker(
        fixture.service(),
        executor,
        "activity-worker",
        &[NOW, LATER],
    )
    .with_activity(layer2_core::ActivityService::new(activity_store.clone()));

    assert_eq!(
        worker
            .run_once(&fixture.launch.id, 0, "effect-audit")
            .await
            .unwrap(),
        EffectRunOutcome::Completed
    );
    let records = activity_store.records.lock().await;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        record.action,
        layer2_core::ActivityAction::TriggerEffectExecute
    );
    assert_eq!(record.outcome, layer2_core::ActivityOutcome::Succeeded);
    assert_eq!(
        record.detail.get(layer2_core::DetailKey::Effect),
        Some("preflight")
    );
    assert_eq!(
        record.detail.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        vec![
            layer2_core::DetailKey::Subject,
            layer2_core::DetailKey::CorrelationId,
            layer2_core::DetailKey::RevisionDigest,
            layer2_core::DetailKey::RepositoryId,
            layer2_core::DetailKey::GithubAccountId,
            layer2_core::DetailKey::ServiceId,
            layer2_core::DetailKey::Effect,
            layer2_core::DetailKey::LaunchState,
            layer2_core::DetailKey::FencingToken,
        ]
    );
    let wire = serde_json::to_string(record).unwrap();
    for forbidden in [
        "prompt",
        "credential",
        "access_token",
        "github_token",
        "file_content",
        "provider_body",
    ] {
        assert!(
            !wire.contains(forbidden),
            "audit record admitted {forbidden}: {wire}"
        );
    }
}

#[tokio::test]
async fn rollout_pause_and_resume_have_distinct_stable_audit_actions() {
    let paused_launch = prompt_pending_launch();
    let paused_store = Arc::new(TestStore::new(paused_launch.clone()));
    let paused_service = Arc::new(
        TriggerService::new(
            paused_store.clone(),
            paused_store.clone(),
            Arc::new(NeverAuthority),
        )
        .with_rollout(Arc::new(rollout_policy(&paused_launch, true, false))),
    );
    let paused_executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&paused_store),
        paused_launch.authority.clone().unwrap(),
    ));
    let activity_store = Arc::new(RecordingActivityStore::default());
    let paused_worker = test_worker(
        paused_service,
        paused_executor,
        "pause-audit-worker",
        &[NOW],
    )
    .with_activity(layer2_core::ActivityService::new(activity_store.clone()));
    assert_eq!(
        paused_worker
            .run_once(
                &paused_launch.id,
                paused_launch.revision_number,
                "pause-audit",
            )
            .await
            .unwrap(),
        EffectRunOutcome::Paused
    );

    let mut resumed_launch = launch_fixture();
    resumed_launch
        .block(
            layer2_core::LaunchBlockedReason::FeaturePaused,
            NOW,
            "old-pause",
        )
        .unwrap();
    let resumed_store = Arc::new(TestStore::new(resumed_launch.clone()));
    let resumed_service = Arc::new(TriggerService::new(
        resumed_store.clone(),
        resumed_store.clone(),
        Arc::new(NeverAuthority),
    ));
    let resumed_executor = Arc::new(InspectingExecutor::complete(
        resumed_store,
        resumed_launch.authority.clone().unwrap(),
    ));
    let resumed_worker = test_worker(
        resumed_service,
        resumed_executor,
        "resume-audit-worker",
        &[NOW, LATER],
    )
    .with_activity(layer2_core::ActivityService::new(activity_store.clone()));
    assert_eq!(
        resumed_worker
            .run_once(
                &resumed_launch.id,
                resumed_launch.revision_number,
                "resume-audit",
            )
            .await
            .unwrap(),
        EffectRunOutcome::Completed
    );

    let records = activity_store.records.lock().await;
    assert_eq!(records.len(), 3);
    assert_eq!(
        records[0].action,
        layer2_core::ActivityAction::TriggerOrchestrationPause
    );
    assert_eq!(
        records[0].detail.get(layer2_core::DetailKey::RolloutReason),
        Some("orchestration_paused")
    );
    assert_eq!(
        records[1].action,
        layer2_core::ActivityAction::TriggerOrchestrationResume
    );
    assert_eq!(
        records[1].detail.get(layer2_core::DetailKey::RolloutReason),
        Some("rollout_permitted")
    );
    assert_eq!(
        records[2].action,
        layer2_core::ActivityAction::TriggerEffectExecute
    );
}

#[tokio::test]
async fn rollout_pause_is_persisted_before_any_effect_and_preserves_the_session() {
    let launch = prompt_pending_launch();
    let original_session = launch.session.clone();
    let original_state = launch.state;
    let store = Arc::new(TestStore::new(launch.clone()));
    let service = Arc::new(
        TriggerService::new(store.clone(), store.clone(), Arc::new(NeverAuthority))
            .with_rollout(Arc::new(rollout_policy(&launch, true, false))),
    );
    let executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&store),
        launch.authority.clone().unwrap(),
    ));
    let worker = test_worker(service, Arc::clone(&executor), "paused-worker", &[NOW]);

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "rollout-paused")
            .await
            .unwrap(),
        EffectRunOutcome::Paused
    );
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);
    let current = store.current();
    assert_eq!(current.state, original_state);
    assert_eq!(current.session, original_session);
    assert_eq!(
        current.blocked_reason,
        Some(layer2_core::LaunchBlockedReason::FeaturePaused)
    );
    assert!(current.lease.is_none());
    assert!(current.events.iter().any(|event| matches!(
        event.kind,
        layer2_core::LaunchEventKind::Blocked {
            reason: layer2_core::LaunchBlockedReason::FeaturePaused
        }
    )));
}

#[tokio::test]
async fn rollout_resume_is_committed_before_the_next_effect_claim() {
    let mut launch = launch_fixture();
    launch
        .block(
            layer2_core::LaunchBlockedReason::FeaturePaused,
            NOW,
            "pause-before-restart",
        )
        .unwrap();
    let store = Arc::new(TestStore::new(launch.clone()));
    let service = Arc::new(TriggerService::new(
        store.clone(),
        store.clone(),
        Arc::new(NeverAuthority),
    ));
    let executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&store),
        launch.authority.clone().unwrap(),
    ));
    let worker = test_worker(
        service,
        Arc::clone(&executor),
        "resumed-worker",
        &[NOW, LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "rollout-resumed")
            .await
            .unwrap(),
        EffectRunOutcome::Completed
    );
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 1);
    let current = store.current();
    assert_eq!(current.blocked_reason, None);
    let resumed = current
        .events
        .iter()
        .position(|event| matches!(event.kind, layer2_core::LaunchEventKind::Resumed { .. }))
        .unwrap();
    let claimed_step = current
        .events
        .iter()
        .position(|event| {
            matches!(
                event.kind,
                layer2_core::LaunchEventKind::StepChanged {
                    step: layer2_core::LaunchStep {
                        status: layer2_core::LaunchStepStatus::Running,
                        ..
                    }
                }
            )
        })
        .unwrap();
    assert!(resumed < claimed_step);
}

#[tokio::test]
async fn takeover_reconciles_a_committed_claim_instead_of_executing_again() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let mut claimed = service
        .acquire_worker(
            &fixture.launch.id,
            0,
            "crashed-worker",
            NOW,
            "2026-08-27T00:00:02Z",
        )
        .await
        .unwrap()
        .unwrap();
    let old_lease = claimed.lease.clone().unwrap();
    let old_claim = claimed
        .claim_next_effect(&old_lease, NOW, "claim-before-crash")
        .unwrap()
        .unwrap();
    service
        .commit_worker(claimed.revision_number, &old_lease, claimed)
        .await
        .unwrap();

    let executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&fixture.store),
        fixture.launch.authority.clone().unwrap(),
    ));
    let worker = test_worker(
        service,
        executor.clone(),
        "replacement-worker",
        &["2026-08-27T00:00:03Z", "2026-08-27T00:00:04Z"],
    );
    let outcome = worker
        .run_once(&fixture.launch.id, 2, "takeover")
        .await
        .unwrap();

    assert_eq!(outcome, EffectRunOutcome::Completed);
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        executor.intent.lock().unwrap().as_deref(),
        Some(old_claim.intent_key.as_str())
    );
}

#[tokio::test]
async fn crash_before_authority_admission_allows_only_a_freshly_admitted_execute() {
    let fixture = fork_fixture();
    let service = fixture.service();
    let mut claimed = service
        .acquire_worker(
            &fixture.launch.id,
            fixture.launch.revision_number,
            "crashed-before-admission",
            NOW,
            "2026-08-27T00:00:02Z",
        )
        .await
        .unwrap()
        .unwrap();
    let old_lease = claimed.lease.clone().unwrap();
    let old_claim = claimed
        .claim_next_effect(&old_lease, NOW, "claim-before-admission")
        .unwrap()
        .unwrap();
    assert_eq!(old_claim.kind, LaunchEffectKind::Fork);
    let stranded = service
        .commit_worker(claimed.revision_number, &old_lease, claimed)
        .await
        .unwrap();
    assert!(stranded
        .effects
        .last()
        .unwrap()
        .authority_admission
        .is_none());

    let executor = Arc::new(AuthorityExecutor::new(false));
    let worker = test_worker(
        service,
        Arc::clone(&executor),
        "fresh-admission-worker",
        &[
            "2026-08-27T00:00:03Z",
            "2026-08-27T00:00:04Z",
            "2026-08-27T00:00:05Z",
        ],
    );
    assert_eq!(
        worker
            .run_once(
                &fixture.launch.id,
                stranded.revision_number,
                "resume-before-admission",
            )
            .await
            .unwrap(),
        EffectRunOutcome::Completed
    );
    assert_eq!(executor.admit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.provider_write_calls.load(Ordering::SeqCst), 1);
    let completed = fixture.store.current();
    assert_eq!(
        completed.effects.last().unwrap().attempt,
        old_claim.attempt + 1
    );
    assert!(completed
        .effects
        .last()
        .unwrap()
        .authority_admission
        .is_some());
}

#[tokio::test]
async fn crash_after_authority_admission_reconciles_whether_or_not_the_write_started() {
    for external_write_started in [false, true] {
        let fixture = fork_fixture();
        let service = fixture.service();
        let mut claimed = service
            .acquire_worker(
                &fixture.launch.id,
                fixture.launch.revision_number,
                "crashed-after-admission",
                NOW,
                "2026-08-27T00:00:02Z",
            )
            .await
            .unwrap()
            .unwrap();
        let old_lease = claimed.lease.clone().unwrap();
        let old_claim = claimed
            .claim_next_effect(&old_lease, NOW, "claim-before-admission")
            .unwrap()
            .unwrap();
        let mut admitted = service
            .commit_worker(claimed.revision_number, &old_lease, claimed)
            .await
            .unwrap();
        let admission = AuthorityExecutor::admission(&admitted, &old_claim, false);
        admitted
            .record_effect_authority_admission(&old_lease, &old_claim, admission.clone(), LATER)
            .unwrap();
        let admitted = service
            .commit_worker(admitted.revision_number, &old_lease, admitted)
            .await
            .unwrap();

        let executor = Arc::new(AuthorityExecutor::new(false));
        if external_write_started {
            let _ = executor.execute(&admitted, &old_claim).await;
        }
        let writes_before_takeover = executor.provider_write_calls.load(Ordering::SeqCst);
        let executes_before_takeover = executor.execute_calls.load(Ordering::SeqCst);
        let worker = test_worker(
            service,
            Arc::clone(&executor),
            "reconciliation-worker",
            &["2026-08-27T00:00:03Z", "2026-08-27T00:00:04Z"],
        );
        assert_eq!(
            worker
                .run_once(
                    &fixture.launch.id,
                    admitted.revision_number,
                    "resume-after-admission",
                )
                .await
                .unwrap(),
            EffectRunOutcome::Completed
        );
        assert_eq!(executor.admit_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            executor.execute_calls.load(Ordering::SeqCst),
            executes_before_takeover
        );
        assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            executor.provider_write_calls.load(Ordering::SeqCst),
            writes_before_takeover
        );
        let completed = fixture.store.current();
        let record = completed.effects.last().unwrap();
        assert_eq!(record.state, layer2_core::EffectState::Succeeded);
        assert_eq!(record.authority_admission, Some(admission.clone()));
        assert_eq!(admission.fence, old_claim.fence);
        assert_ne!(record.fence, admission.fence);
    }
}

#[tokio::test]
async fn lost_authority_admission_cas_never_calls_execute_or_the_provider() {
    let fixture = fork_fixture();
    fixture.store.fail_replace_on(2);
    let executor = Arc::new(AuthorityExecutor::new(false));
    let worker = test_worker(
        fixture.service(),
        Arc::clone(&executor),
        "admission-cas-loser",
        &[NOW, LATER],
    );

    let error = worker
        .run_once(
            &fixture.launch.id,
            fixture.launch.revision_number,
            "lose-admission-cas",
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        TriggerServiceError::Store(TriggerStoreError::Unavailable(_))
    ));
    assert_eq!(executor.admit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.provider_write_calls.load(Ordering::SeqCst), 0);
    let stranded = fixture.store.current();
    assert_eq!(
        stranded.effects.last().unwrap().state,
        layer2_core::EffectState::Claimed
    );
    assert!(stranded
        .effects
        .last()
        .unwrap()
        .authority_admission
        .is_none());
}

#[tokio::test]
async fn stale_fence_authority_admission_never_reaches_the_execute_boundary() {
    let fixture = fork_fixture();
    let executor = Arc::new(AuthorityExecutor::new(true));
    let worker = test_worker(
        fixture.service(),
        Arc::clone(&executor),
        "stale-admission-worker",
        &[NOW, LATER],
    );

    assert!(matches!(
        worker
            .run_once(
                &fixture.launch.id,
                fixture.launch.revision_number,
                "stale-admission",
            )
            .await,
        Err(TriggerServiceError::Validation(_))
    ));
    assert_eq!(executor.admit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);
    assert_eq!(executor.provider_write_calls.load(Ordering::SeqCst), 0);
    assert!(fixture
        .store
        .current()
        .effects
        .last()
        .unwrap()
        .authority_admission
        .is_none());
}

#[tokio::test]
async fn every_durable_publication_phase_requires_a_fresh_authority_admission() {
    let launch = super::super::github_effects::tests::publication_launch_for_worker_test();
    let store = Arc::new(TestStore::new(launch.clone()));
    let service = Arc::new(TriggerService::new(
        store.clone(),
        store.clone(),
        Arc::new(NeverAuthority),
    ));
    let executor = Arc::new(PublicationAuthorityExecutor::default());
    let worker = test_worker(
        service,
        Arc::clone(&executor),
        "publication-phase-worker",
        &[
            "2026-08-27T00:00:03Z",
            "2026-08-27T00:00:04Z",
            "2026-08-27T00:00:05Z",
            "2026-08-27T00:00:06Z",
            "2026-08-27T00:00:07Z",
            "2026-08-27T00:00:08Z",
            "2026-08-27T00:00:09Z",
            "2026-08-27T00:00:10Z",
            "2026-08-27T00:00:11Z",
        ],
    );

    for (phase, expected) in [
        EffectRunOutcome::Deferred,
        EffectRunOutcome::Deferred,
        EffectRunOutcome::Completed,
    ]
    .into_iter()
    .enumerate()
    {
        let current = store.current();
        assert_eq!(
            worker
                .run_once(
                    &launch.id,
                    current.revision_number,
                    &format!("publication-phase-{}", phase + 1),
                )
                .await
                .unwrap(),
            expected
        );
    }

    assert_eq!(executor.admit_calls.load(Ordering::SeqCst), 3);
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 3);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);
    assert_eq!(*executor.admitted_fences.lock().unwrap(), vec![1, 2, 3]);
    let completed = store.current();
    assert_eq!(
        completed.contribution.as_ref().unwrap().state,
        layer2_core::PublicationState::Published
    );
    assert_eq!(
        completed.branch.as_ref().unwrap().current_head,
        "d".repeat(40)
    );
    let record = completed.effects.last().unwrap();
    assert_eq!(record.state, layer2_core::EffectState::Succeeded);
    assert_eq!(
        record.authority_admission.as_ref().unwrap().fence,
        record.fence
    );
}

#[tokio::test]
async fn result_observed_after_lease_expiry_is_rejected_and_then_reconciled() {
    let fixture = Fixture::new();
    let executor = Arc::new(InspectingExecutor::complete(
        Arc::clone(&fixture.store),
        fixture.launch.authority.clone().unwrap(),
    ));
    let expired_worker = test_worker(
        fixture.service(),
        executor.clone(),
        "slow-worker",
        &[NOW, "2026-08-27T00:02:01Z"],
    );

    let error = expired_worker
        .run_once(&fixture.launch.id, 0, "slow-result")
        .await
        .unwrap_err();
    assert!(matches!(error, TriggerServiceError::Validation(_)));
    let stranded = fixture.store.current();
    assert_eq!(stranded.revision_number, 2);
    assert_eq!(stranded.effects[0].state, layer2_core::EffectState::Claimed);

    let replacement = test_worker(
        fixture.service(),
        executor.clone(),
        "replacement-worker",
        &["2026-08-27T00:02:02Z", "2026-08-27T00:02:03Z"],
    );
    assert_eq!(
        replacement
            .run_once(
                &fixture.launch.id,
                stranded.revision_number,
                "reconcile-slow"
            )
            .await
            .unwrap(),
        EffectRunOutcome::Completed
    );
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_ambiguous_write_waits_for_read_only_reconciliation() {
    let fixture = Fixture::new();
    let executor = Arc::new(InspectingExecutor::ambiguous(
        Arc::clone(&fixture.store),
        fixture.launch.authority.clone().unwrap(),
    ));
    let worker = test_worker(
        fixture.service(),
        executor.clone(),
        "worker-a",
        &[
            NOW,
            LATER,
            "2026-08-27T00:00:02Z",
            "2026-08-27T00:00:10Z",
            "2026-08-27T00:00:11Z",
        ],
    );
    assert_eq!(
        worker
            .run_once(&fixture.launch.id, 0, "ambiguous")
            .await
            .unwrap(),
        EffectRunOutcome::Deferred
    );
    let after_ambiguous = fixture.store.current();
    assert_eq!(
        after_ambiguous.effects[0].state,
        layer2_core::EffectState::Reconciling
    );
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 1);

    assert_eq!(
        worker
            .run_once(
                &fixture.launch.id,
                after_ambiguous.revision_number,
                "early-poll",
            )
            .await
            .unwrap(),
        EffectRunOutcome::NoWork
    );
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 0);

    executor.outcome.store(0, Ordering::SeqCst);
    let due = fixture.store.current();
    assert_eq!(
        worker
            .run_once(&fixture.launch.id, due.revision_number, "reconcile")
            .await
            .unwrap(),
        EffectRunOutcome::Completed
    );
    assert_eq!(executor.execute_calls.load(Ordering::SeqCst), 1);
    assert_eq!(executor.reconcile_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retryable_and_terminal_failures_release_their_worker_lease() {
    let retry_fixture = Fixture::new();
    let retry_executor = Arc::new(InspectingExecutor::retry(
        Arc::clone(&retry_fixture.store),
        retry_fixture.launch.authority.clone().unwrap(),
    ));
    let retry_worker = test_worker(
        retry_fixture.service(),
        retry_executor,
        "retry-worker",
        &[NOW, LATER],
    );
    assert_eq!(
        retry_worker
            .run_once(&retry_fixture.launch.id, 0, "retry")
            .await
            .unwrap(),
        EffectRunOutcome::Deferred
    );
    let retry = retry_fixture.store.current();
    assert_eq!(retry.state, LaunchState::FailedRetryable);
    assert!(retry.lease.is_none());

    let terminal_fixture = Fixture::new();
    let terminal_executor = Arc::new(InspectingExecutor::terminal(
        Arc::clone(&terminal_fixture.store),
        terminal_fixture.launch.authority.clone().unwrap(),
    ));
    let terminal_worker = test_worker(
        terminal_fixture.service(),
        terminal_executor,
        "terminal-worker",
        &[NOW, LATER],
    );
    assert_eq!(
        terminal_worker
            .run_once(&terminal_fixture.launch.id, 0, "terminal")
            .await
            .unwrap(),
        EffectRunOutcome::Failed
    );
    let terminal = terminal_fixture.store.current();
    assert_eq!(terminal.state, LaunchState::FailedTerminal);
    assert!(terminal.lease.is_none());
}

#[tokio::test]
async fn initial_prompt_progress_is_committed_before_the_worker_defers_or_fails() {
    use layer2_core::PromptDeliveryState as State;

    for (state, expected_outcome) in [
        (State::Queued, EffectRunOutcome::Deferred),
        (State::WriteConfirmed, EffectRunOutcome::Deferred),
        (State::RecoveryRequired, EffectRunOutcome::Failed),
    ] {
        let launch = prompt_pending_launch();
        let store = Arc::new(TestStore::new(launch.clone()));
        let service = Arc::new(TriggerService::new(
            store.clone(),
            store.clone(),
            Arc::new(NeverAuthority),
        ));
        let executor = Arc::new(PromptProgressExecutor { state });
        let worker = test_worker(service, executor, "prompt-worker", &[NOW, LATER]);

        assert_eq!(
            worker
                .run_once(&launch.id, 0, "prompt-progress")
                .await
                .unwrap(),
            expected_outcome
        );
        let stored = store.current();
        assert_eq!(stored.delivery.as_ref().unwrap().state, state);
        assert!(stored.lease.is_none());
        assert!(stored.events.iter().any(|event| matches!(
            event.kind,
            layer2_core::LaunchEventKind::DeliveryChanged { state: observed }
                if observed == state
        )));
        match state {
            State::Queued => {
                assert_eq!(stored.state, LaunchState::FailedRetryable);
                assert_eq!(
                    stored.effects[0].state,
                    layer2_core::EffectState::FailedRetryable
                );
            }
            State::WriteConfirmed => {
                assert_eq!(stored.state, LaunchState::PromptPending);
                assert_eq!(
                    stored.effects[0].state,
                    layer2_core::EffectState::Reconciling
                );
            }
            State::RecoveryRequired => {
                assert_eq!(stored.state, LaunchState::FailedRetryable);
                assert_eq!(
                    stored.blocked_reason,
                    Some(layer2_core::LaunchBlockedReason::RecoveryRequired)
                );
                assert_eq!(
                    stored.effects[0].state,
                    layer2_core::EffectState::RecoveryRequired
                );
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn a_closed_progress_phase_is_committed_and_released_for_reconciliation() {
    let launch = prompt_pending_launch();
    let store = Arc::new(TestStore::new(launch.clone()));
    let service = Arc::new(TriggerService::new(
        store.clone(),
        store.clone(),
        Arc::new(NeverAuthority),
    ));
    let executor = Arc::new(ClosedProgressExecutor);
    let worker = test_worker(service, executor, "progress-worker", &[NOW, LATER]);

    assert_eq!(
        worker
            .run_once(&launch.id, 0, "closed-progress")
            .await
            .unwrap(),
        EffectRunOutcome::Deferred
    );
    let stored = store.current();
    assert_eq!(
        stored.delivery.as_ref().unwrap().state,
        layer2_core::PromptDeliveryState::Queued
    );
    assert_eq!(stored.effects[0].state, layer2_core::EffectState::Claimed);
    assert!(stored.lease.is_none());
}

struct ClosedProgressExecutor;

#[async_trait]
impl EffectExecutor for ClosedProgressExecutor {
    async fn execute(&self, launch: &Launch, _claim: &EffectClaim) -> EffectExecution {
        EffectExecution::Progressed {
            result: LaunchEffectResult::InitialDispatch {
                delivery: prompt_delivery(launch, layer2_core::PromptDeliveryState::Queued),
            },
        }
    }

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.execute(launch, claim).await
    }
}

struct PromptProgressExecutor {
    state: layer2_core::PromptDeliveryState,
}

#[async_trait]
impl EffectExecutor for PromptProgressExecutor {
    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.answer(launch, claim)
    }

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.answer(launch, claim)
    }
}

impl PromptProgressExecutor {
    fn answer(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let delivery = prompt_delivery(launch, self.state);
        let result = LaunchEffectResult::InitialDispatch { delivery };
        let failure = LaunchFailure {
            class: match self.state {
                layer2_core::PromptDeliveryState::RecoveryRequired => {
                    layer2_core::FailureClass::RecoveryRequired
                }
                _ => layer2_core::FailureClass::RetryableTransient,
            },
            code: "prompt_progress_test".into(),
            detail: "The prompt remains in a durable intermediate state.".into(),
            remediation: "Reconcile the same prompt identity.".into(),
            correlation_id: claim.intent_key.clone(),
            retry_at: None,
            observed_at: LATER.into(),
        };
        match self.state {
            layer2_core::PromptDeliveryState::Queued => EffectExecution::RetryWithProgress {
                result,
                failure,
                retry_at: "2026-08-27T00:00:10Z".into(),
            },
            layer2_core::PromptDeliveryState::WriteConfirmed => {
                EffectExecution::ReconcileWithProgress {
                    result,
                    failure,
                    reconcile_at: "2026-08-27T00:00:10Z".into(),
                    external_resource_ids: BTreeMap::new(),
                }
            }
            layer2_core::PromptDeliveryState::RecoveryRequired => {
                EffectExecution::FailedWithProgress { result, failure }
            }
            _ => unreachable!(),
        }
    }
}

fn prompt_pending_launch() -> Launch {
    let mut launch = launch_fixture();
    launch.state = LaunchState::PromptPending;
    launch.session = Some(layer2_core::SessionLink {
        session_id: layer2_core::SessionId("sesn-prompt-worker".into()),
        environment_profile_id: "envp-prompt-worker".into(),
        runtime_generation: 1,
        created_at: NOW.into(),
    });
    launch.reach = Some(layer2_core::ReachStatus {
        service_id: "service-1".into(),
        state: layer2_core::CapabilityRevocationState::Active,
        policy_generation: 1,
        policy_fence: 1,
        runtime_audience: "runtime-prompt-worker".into(),
        expires_at_epoch_seconds: seconds_from_rfc3339("2026-08-27T00:05:00Z").unwrap(),
        numeric_binding_observed_at: NOW.into(),
        refreshed_at: NOW.into(),
        revocation_requested_at: None,
        revocation_effective_at: None,
        last_refusal: None,
        tombstone_acknowledged: false,
        agent_key_revocation_confirmed: false,
    });
    launch.effects.clear();
    launch.delivery = None;
    launch
}

fn prompt_delivery(
    launch: &Launch,
    state: layer2_core::PromptDeliveryState,
) -> layer2_core::PromptDelivery {
    layer2_core::PromptDelivery {
        launch_id: launch.id.clone(),
        session_id: launch.session.as_ref().unwrap().session_id.clone(),
        revision_digest: launch.revision_digest.clone(),
        origin_key: format!(
            "cma-trigger:{}:{}:initial",
            launch.id.0, launch.revision_digest
        ),
        payload_hash: "a".repeat(64),
        state,
        attempt: u32::from(state != layer2_core::PromptDeliveryState::Queued),
        recovery_generation: 0,
        dispatch_fence_sequence: (state != layer2_core::PromptDeliveryState::Queued).then_some(42),
        queued_at: NOW.into(),
        updated_at: LATER.into(),
    }
}

struct WaitingExecutor;

#[async_trait]
impl EffectExecutor for WaitingExecutor {
    async fn execute(&self, _launch: &Launch, _claim: &EffectClaim) -> EffectExecution {
        EffectExecution::WaitLater {
            wait_at: "2026-08-27T00:00:02Z".into(),
            safe_detail: "The deterministic session is recovering.".into(),
            remediation: "CMA will check the same session again.".into(),
            external_resource_ids: BTreeMap::from([(
                "session_id".into(),
                "sesn_trigger_wait".into(),
            )]),
        }
    }

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.execute(launch, claim).await
    }
}

struct InspectingExecutor {
    store: Arc<TestStore>,
    authority: AuthorityProvenance,
    execute_calls: AtomicUsize,
    reconcile_calls: AtomicUsize,
    saw_committed_claim: AtomicUsize,
    outcome: AtomicUsize,
    intent: Mutex<Option<String>>,
}

impl InspectingExecutor {
    fn complete(store: Arc<TestStore>, authority: AuthorityProvenance) -> Self {
        Self {
            store,
            authority,
            execute_calls: AtomicUsize::new(0),
            reconcile_calls: AtomicUsize::new(0),
            saw_committed_claim: AtomicUsize::new(0),
            outcome: AtomicUsize::new(0),
            intent: Mutex::new(None),
        }
    }

    fn ambiguous(store: Arc<TestStore>, authority: AuthorityProvenance) -> Self {
        let value = Self::complete(store, authority);
        value.outcome.store(1, Ordering::SeqCst);
        value
    }

    fn retry(store: Arc<TestStore>, authority: AuthorityProvenance) -> Self {
        let value = Self::complete(store, authority);
        value.outcome.store(2, Ordering::SeqCst);
        value
    }

    fn terminal(store: Arc<TestStore>, authority: AuthorityProvenance) -> Self {
        let value = Self::complete(store, authority);
        value.outcome.store(3, Ordering::SeqCst);
        value
    }

    fn answer(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        let stored = self.store.current();
        if stored.revision_number == launch.revision_number
            && stored.effects.iter().any(|record| {
                record.intent_key == claim.intent_key
                    && matches!(
                        record.state,
                        layer2_core::EffectState::Claimed | layer2_core::EffectState::Reconciling
                    )
            })
        {
            self.saw_committed_claim.fetch_add(1, Ordering::SeqCst);
        }
        *self.intent.lock().unwrap() = Some(claim.intent_key.clone());
        match self.outcome.load(Ordering::SeqCst) {
            1 => EffectExecution::ReconcileLater {
                failure: LaunchFailure {
                    class: layer2_core::FailureClass::AmbiguousExternal,
                    code: "provider_outcome_unknown".into(),
                    detail: "The provider outcome is not known yet.".into(),
                    remediation: "CMA will query the provider before continuing.".into(),
                    correlation_id: "provider-request".into(),
                    retry_at: None,
                    observed_at: LATER.into(),
                },
                reconcile_at: "2026-08-27T00:00:10Z".into(),
                external_resource_ids: BTreeMap::from([(
                    "provider_request_id".into(),
                    "fork-request-1".into(),
                )]),
            },
            2 => EffectExecution::RetryLater {
                failure: LaunchFailure {
                    class: layer2_core::FailureClass::RetryableTransient,
                    code: "provider_temporarily_unavailable".into(),
                    detail: "The provider is temporarily unavailable.".into(),
                    remediation: "CMA will retry after the shown deadline.".into(),
                    correlation_id: "provider-request".into(),
                    retry_at: None,
                    observed_at: LATER.into(),
                },
                retry_at: "2026-08-27T00:00:10Z".into(),
            },
            3 => EffectExecution::Failed {
                failure: LaunchFailure {
                    class: layer2_core::FailureClass::TerminalValidation,
                    code: "repository_deleted".into(),
                    detail: "The upstream repository no longer exists.".into(),
                    remediation: "Use a Trigger revision for an available repository.".into(),
                    correlation_id: "provider-request".into(),
                    retry_at: None,
                    observed_at: LATER.into(),
                },
            },
            _ => EffectExecution::Completed {
                result: LaunchEffectResult::Preflight {
                    connection: connection(),
                    authority: self.authority.clone(),
                    source_sha: "a".repeat(40),
                    workflow_paths: Vec::new(),
                },
                safe_detail: "Authority is current.".into(),
                external_resource_ids: BTreeMap::new(),
            },
        }
    }
}

#[async_trait]
impl EffectExecutor for InspectingExecutor {
    async fn execute(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.execute_calls.fetch_add(1, Ordering::SeqCst);
        self.answer(launch, claim)
    }

    async fn reconcile(&self, launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        self.answer(launch, claim)
    }
}

struct AuthorityExecutor {
    stale_admission: bool,
    admit_calls: AtomicUsize,
    execute_calls: AtomicUsize,
    reconcile_calls: AtomicUsize,
    provider_write_calls: AtomicUsize,
}

impl AuthorityExecutor {
    fn new(stale_admission: bool) -> Self {
        Self {
            stale_admission,
            admit_calls: AtomicUsize::new(0),
            execute_calls: AtomicUsize::new(0),
            reconcile_calls: AtomicUsize::new(0),
            provider_write_calls: AtomicUsize::new(0),
        }
    }

    fn admission(
        launch: &Launch,
        claim: &EffectClaim,
        stale_fence: bool,
    ) -> AuthorityAdmissionSnapshot {
        let mut admitted_claim = claim.clone();
        if stale_fence {
            admitted_claim.fence = claim.fence.saturating_add(1);
        }
        AuthorityAdmissionSnapshot::for_effect(
            launch,
            &admitted_claim,
            launch.connection.clone().unwrap(),
            launch.authority.clone().unwrap(),
            launch.revision.content.upstream.repository.clone(),
            None,
            None,
            LATER,
        )
        .unwrap()
    }

    fn completed_fork(launch: &Launch) -> EffectExecution {
        let upstream = &launch.revision.content.upstream.repository;
        let connection = launch.connection.as_ref().unwrap();
        let repository = RepositoryIdentity {
            repository_id: 11,
            repository_node_id: "R_fork".into(),
            owner_id: connection.account.account_id,
            owner_node_id: connection.account.account_node_id.clone(),
            canonical_owner: connection.account.login.clone(),
            canonical_name: upstream.canonical_name.clone(),
            visibility: upstream.visibility,
            allow_forking: true,
            default_branch: upstream.default_branch.clone(),
            selected_ref: upstream.selected_ref.clone(),
            fork_parent_id: Some(upstream.repository_id),
            fork_source_id: Some(upstream.fork_source_id.unwrap_or(upstream.repository_id)),
        };
        EffectExecution::Completed {
            result: LaunchEffectResult::Fork {
                fork: layer2_core::ForkRecord {
                    repository,
                    reused: false,
                    source_revision: launch.source_sha.clone(),
                    canonical_url: "https://github.com/contributor/project".into(),
                    ready_at: LATER.into(),
                },
            },
            safe_detail: "The exact admitted fork is ready.".into(),
            external_resource_ids: BTreeMap::new(),
        }
    }
}

#[async_trait]
impl EffectExecutor for AuthorityExecutor {
    async fn admit(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<Option<AuthorityAdmissionSnapshot>, EffectExecution> {
        self.admit_calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Self::admission(launch, claim, self.stale_admission)))
    }

    async fn execute(&self, launch: &Launch, _claim: &EffectClaim) -> EffectExecution {
        self.execute_calls.fetch_add(1, Ordering::SeqCst);
        self.provider_write_calls.fetch_add(1, Ordering::SeqCst);
        Self::completed_fork(launch)
    }

    async fn reconcile(&self, launch: &Launch, _claim: &EffectClaim) -> EffectExecution {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        Self::completed_fork(launch)
    }
}

#[derive(Default)]
struct PublicationAuthorityExecutor {
    admit_calls: AtomicUsize,
    execute_calls: AtomicUsize,
    reconcile_calls: AtomicUsize,
    admitted_fences: Mutex<Vec<u64>>,
}

#[async_trait]
impl EffectExecutor for PublicationAuthorityExecutor {
    async fn admit(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<Option<AuthorityAdmissionSnapshot>, EffectExecution> {
        self.admit_calls.fetch_add(1, Ordering::SeqCst);
        self.admitted_fences.lock().unwrap().push(claim.fence);
        Ok(Some(
            AuthorityAdmissionSnapshot::for_effect(
                launch,
                claim,
                launch.connection.clone().unwrap(),
                launch.authority.clone().unwrap(),
                launch.revision.content.upstream.repository.clone(),
                Some(launch.fork.as_ref().unwrap().repository.clone()),
                launch.branch.clone(),
                LATER,
            )
            .unwrap(),
        ))
    }

    async fn execute(&self, launch: &Launch, _claim: &EffectClaim) -> EffectExecution {
        self.execute_calls.fetch_add(1, Ordering::SeqCst);
        let mut contribution = launch.contribution.clone().unwrap();
        match contribution.state {
            layer2_core::PublicationState::IntentRecorded => {
                contribution.state = layer2_core::PublicationState::ObjectsWritten;
                contribution.base_tree_id = Some("b".repeat(40));
                contribution.tree_id = Some("c".repeat(40));
                contribution.commit_id = Some("d".repeat(40));
                contribution.object_ids = vec![launch.diff.as_ref().unwrap().files[0]
                    .new_provider_object_id
                    .clone()
                    .unwrap()];
                contribution.updated_at = LATER.into();
                EffectExecution::Progressed {
                    result: LaunchEffectResult::BranchPublication { contribution },
                }
            }
            layer2_core::PublicationState::ObjectsWritten => {
                contribution.state = layer2_core::PublicationState::RefUpdatePending;
                contribution.updated_at = "2026-08-27T00:00:02Z".into();
                EffectExecution::Progressed {
                    result: LaunchEffectResult::BranchPublication { contribution },
                }
            }
            layer2_core::PublicationState::RefUpdatePending => {
                contribution.state = layer2_core::PublicationState::Published;
                contribution.resulting_head = contribution.commit_id.clone();
                contribution.updated_at = "2026-08-27T00:00:03Z".into();
                let mut resources = BTreeMap::new();
                resources.insert(
                    "base_tree_id".into(),
                    contribution.base_tree_id.clone().unwrap(),
                );
                resources.insert("tree_id".into(), contribution.tree_id.clone().unwrap());
                resources.insert("commit_id".into(), contribution.commit_id.clone().unwrap());
                resources.insert(
                    "resulting_head".into(),
                    contribution.resulting_head.clone().unwrap(),
                );
                EffectExecution::Completed {
                    result: LaunchEffectResult::BranchPublication { contribution },
                    safe_detail: "The exact admitted publication is complete.".into(),
                    external_resource_ids: resources,
                }
            }
            state => panic!("unexpected publication phase in worker test: {state:?}"),
        }
    }

    async fn reconcile(&self, _launch: &Launch, claim: &EffectClaim) -> EffectExecution {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        EffectExecution::Failed {
            failure: LaunchFailure {
                class: layer2_core::FailureClass::RecoveryRequired,
                code: "unexpected_publication_reconciliation".into(),
                detail: "The test publication phase was not freshly admitted.".into(),
                remediation: "Inspect the phase claim.".into(),
                correlation_id: claim.intent_key.clone(),
                retry_at: None,
                observed_at: LATER.into(),
            },
        }
    }
}

struct Fixture {
    launch: Launch,
    store: Arc<TestStore>,
}

impl Fixture {
    fn new() -> Self {
        let owner = SubjectId("contributor".into());
        let revision = revision(SubjectId("publisher".into()));
        let mut launch = Launch::new(owner.clone(), revision, NOW, "create").unwrap();
        launch
            .select_connection(connection(), NOW, "connection")
            .unwrap();
        launch.authority = Some(provenance(owner.clone()));
        admit_launch_for_worker_test(&mut launch);
        launch
            .start(
                layer2_core::Confirmation {
                    action: layer2_core::ConfirmationAction::Start,
                    subject: owner,
                    snapshot_digest: launch.effective.digest.clone(),
                    confirmed_at: NOW.into(),
                    idempotency_key_hash: "start-key-hash".into(),
                },
                NOW,
                "start",
            )
            .unwrap();
        Self {
            store: Arc::new(TestStore::new(launch.clone())),
            launch,
        }
    }

    fn service(&self) -> Arc<TriggerService> {
        Arc::new(TriggerService::new(
            self.store.clone(),
            self.store.clone(),
            Arc::new(NeverAuthority),
        ))
    }
}

fn fork_fixture() -> Fixture {
    let mut launch = Fixture::new().launch;
    launch.state = LaunchState::ForkPending;
    launch.effects.clear();
    launch.lease = None;
    let store = Arc::new(TestStore::new(launch.clone()));
    Fixture { launch, store }
}

fn admit_launch_for_worker_test(launch: &mut Launch) {
    let quota = QuotaSnapshot {
        live_sessions: 0,
        live_session_limit: Some(5),
        reserved_cpu_millis: 0,
        reserved_memory_mib: 0,
        free_cpu_millis: Some(4_000),
        free_memory_mib: Some(8_192),
        checked_at: NOW.into(),
    };
    launch
        .apply_review_admission(LaunchReviewAdmission {
            pricing: PricingSnapshot {
                policy_reference: layer2_core::CMA_TRIGGER_ZERO_COST_POLICY_V1.into(),
                billing_mode: BillingMode::ZeroCost,
                rate_card: None,
                rate: None,
                currency: layer2_core::CURRENCY.into(),
                cpu_millis: launch.effective.resource_size.cpu_millis(),
                memory_mib: launch.effective.resource_size.memory_mib(),
                micros_per_hour: 0,
                estimated_micros: 0,
                quoted_at: NOW.into(),
                expires_at: launch.draft_expires_at.clone(),
            },
            runtime: RuntimeAdmissionSnapshot {
                kernel: layer2_core::TriggerKernel::Codex,
                runtime_version: "cma-trigger-worker-test@sha256:admitted".into(),
                supported_capabilities: launch
                    .revision
                    .content
                    .session
                    .required_capabilities
                    .clone(),
                observed_at: NOW.into(),
            },
            quota: quota.clone(),
        })
        .unwrap();
    launch
        .record_start_admission(StartAdmissionSnapshot {
            runtime_version: launch
                .runtime_admission
                .as_ref()
                .unwrap()
                .runtime_version
                .clone(),
            pricing_policy_reference: launch.pricing.as_ref().unwrap().policy_reference.clone(),
            rate_card: None,
            quota,
            checked_at: NOW.into(),
        })
        .unwrap();
}

pub(crate) fn launch_fixture() -> Launch {
    Fixture::new().launch
}

pub(crate) fn service_for_launch(launch: Launch) -> Arc<TriggerService> {
    let store = Arc::new(TestStore::new(launch));
    Arc::new(TriggerService::new(
        store.clone(),
        store,
        Arc::new(NeverAuthority),
    ))
}

pub(crate) fn service_for_launch_with_rollout(
    launch: Launch,
    rollout: TriggerRolloutPolicy,
) -> Arc<TriggerService> {
    let store = Arc::new(TestStore::new(launch));
    Arc::new(
        TriggerService::new(store.clone(), store, Arc::new(NeverAuthority))
            .with_rollout(Arc::new(rollout)),
    )
}

pub(crate) fn rollout_policy(
    launch: &Launch,
    orchestration_paused: bool,
    capability_kill_switch: bool,
) -> TriggerRolloutPolicy {
    TriggerRolloutPolicy::new(
        true,
        orchestration_paused,
        capability_kill_switch,
        [launch.revision.content.publisher_subject_id.0.clone()],
        [launch.revision.content.upstream.repository.repository_id],
        100,
    )
    .unwrap()
}

struct TestStore {
    launch: Mutex<Launch>,
    replace_calls: AtomicUsize,
    fail_replace_on: Mutex<Option<usize>>,
}

impl TestStore {
    fn new(launch: Launch) -> Self {
        Self {
            launch: Mutex::new(launch),
            replace_calls: AtomicUsize::new(0),
            fail_replace_on: Mutex::new(None),
        }
    }

    fn current(&self) -> Launch {
        self.launch.lock().unwrap().clone()
    }

    fn fail_replace_on(&self, call: usize) {
        *self.fail_replace_on.lock().unwrap() = Some(call);
    }
}

#[async_trait]
impl LaunchStore for TestStore {
    async fn insert_or_get_current(&self, launch: &Launch) -> Result<Launch, TriggerStoreError> {
        Ok(launch.clone())
    }

    async fn get_current_launch(
        &self,
        owner: &SubjectId,
        revision: &TriggerRevisionId,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let launch = self.current();
        Ok((&launch.owner == owner && &launch.revision.id == revision).then_some(launch))
    }

    async fn insert_new(&self, _launch: &Launch) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::AlreadyExists)
    }

    async fn expire_current_and_insert(
        &self,
        _expired: &Launch,
        _replacement: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        Err(TriggerStoreError::AlreadyExists)
    }

    async fn get_launch(
        &self,
        id: &layer2_core::LaunchId,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let launch = self.current();
        Ok((launch.id == *id).then_some(launch))
    }

    async fn list_launches(
        &self,
        _query: &LaunchQuery,
    ) -> Result<TriggerPage<Launch>, TriggerStoreError> {
        Ok(TriggerPage {
            items: vec![self.current()],
            next: None,
        })
    }

    async fn replace_launch(
        &self,
        expected_revision: u64,
        expected_fence: Option<u64>,
        launch: &Launch,
    ) -> Result<Launch, TriggerStoreError> {
        let call = self.replace_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if *self.fail_replace_on.lock().unwrap() == Some(call) {
            return Err(TriggerStoreError::Unavailable(
                "injected authority-admission CAS loss".into(),
            ));
        }
        let mut stored = self.launch.lock().unwrap();
        if stored.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision_number,
            });
        }
        if expected_fence.is_some_and(|fence| {
            fence != stored.fencing_counter
                || stored.lease.as_ref().map(|lease| lease.fencing_token) != Some(fence)
        }) {
            return Err(TriggerStoreError::StaleFence {
                expected: expected_fence.unwrap(),
                actual: stored.fencing_counter,
            });
        }
        *stored = launch.clone();
        Ok(launch.clone())
    }

    async fn acquire_lease(
        &self,
        id: &layer2_core::LaunchId,
        expected_revision: u64,
        holder: &str,
        acquired_at: &str,
        expires_at: &str,
    ) -> Result<Option<Launch>, TriggerStoreError> {
        let mut launch = self.launch.lock().unwrap();
        if launch.id != *id
            || launch.revision_number != expected_revision
            || launch
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at.as_str() > acquired_at)
        {
            return Ok(None);
        }
        launch.revision_number += 1;
        launch.fencing_counter += 1;
        launch.lease = Some(WorkerLease {
            holder: holder.into(),
            fencing_token: launch.fencing_counter,
            acquired_at: acquired_at.into(),
            expires_at: expires_at.into(),
        });
        Ok(Some(launch.clone()))
    }

    async fn release_lease(
        &self,
        id: &layer2_core::LaunchId,
        expected_revision: u64,
        lease: &WorkerLease,
    ) -> Result<Launch, TriggerStoreError> {
        let mut launch = self.launch.lock().unwrap();
        if launch.id != *id || launch.revision_number != expected_revision {
            return Err(TriggerStoreError::RevisionConflict {
                expected: expected_revision,
                actual: launch.revision_number,
            });
        }
        if launch.lease.as_ref() != Some(lease) {
            return Err(TriggerStoreError::StaleFence {
                expected: lease.fencing_token,
                actual: launch.fencing_counter,
            });
        }
        launch.revision_number += 1;
        launch.lease = None;
        Ok(launch.clone())
    }
}

#[async_trait]
impl TriggerStore for TestStore {
    async fn insert_draft(&self, _draft: &TriggerDraft) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::AlreadyExists)
    }

    async fn get_draft(&self, _id: &TriggerId) -> Result<Option<TriggerDraft>, TriggerStoreError> {
        Ok(None)
    }

    async fn list_drafts_for(
        &self,
        _owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerDraft>, TriggerStoreError> {
        Ok(TriggerPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn replace_draft(
        &self,
        _expected_version: u64,
        _draft: &TriggerDraft,
    ) -> Result<TriggerDraft, TriggerStoreError> {
        Err(TriggerStoreError::AlreadyExists)
    }

    async fn publish_revision(
        &self,
        _expected_draft_version: u64,
        _draft: &TriggerDraft,
        _revision: &TriggerRevision,
    ) -> Result<PublishedRevision, TriggerStoreError> {
        Err(TriggerStoreError::AlreadyExists)
    }

    async fn get_revision(
        &self,
        _id: &TriggerRevisionId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok(None)
    }

    async fn get_public_revision(
        &self,
        _id: &PublicTriggerId,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok(None)
    }

    async fn list_revisions_for(
        &self,
        _owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<TriggerRevision>, TriggerStoreError> {
        Ok(TriggerPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn retire_revision(
        &self,
        _id: &TriggerRevisionId,
        _owner: &SubjectId,
        _retired_at: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        Ok(None)
    }
}

struct NeverAuthority;

#[async_trait]
impl TriggerAuthorityVerifier for NeverAuthority {
    async fn verify_publisher(
        &self,
        _subject: &SubjectId,
        _content: &TriggerRevisionContent,
        _selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }

    async fn verify_launch(
        &self,
        _subject: &SubjectId,
        _revision: &TriggerRevision,
        _selected_service_id: &str,
        _previous: Option<&ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError> {
        Err(TriggerServiceError::NotFound)
    }
}

fn revision(owner: SubjectId) -> TriggerRevision {
    let content = content(owner);
    TriggerRevision {
        id: TriggerRevisionId::new(),
        public_id: PublicTriggerId::new(),
        digest: content.digest().unwrap(),
        publisher_authority: PublisherAuthority {
            subject: content.publisher_subject_id.clone(),
            subject_token_generation: 1,
            connection: connection(),
            repository: upstream(),
            role: layer2_core::PublisherRepositoryRole::Owner,
            verified_at: NOW.into(),
        },
        content,
        availability: layer2_core::TriggerAvailability::Published,
        published_at: NOW.into(),
        retired_at: None,
    }
}

fn content(owner: SubjectId) -> TriggerRevisionContent {
    TriggerRevisionContent {
        schema_version: TRIGGER_SCHEMA_VERSION,
        trigger_id: TriggerId::new(),
        revision_number: 1,
        publisher_subject_id: owner,
        publisher_display_identity: "Publisher".into(),
        upstream: UpstreamRevision {
            repository: upstream(),
            source_mode: SourceMode::PinnedCommit,
            observed_source_sha: Some("a".repeat(40)),
            contribution_base_ref: "main".into(),
        },
        task: TriggerTask {
            public_summary: "Fix one issue".into(),
            prompt_template: "Fix {{topic}}".into(),
            renderer_version: layer2_core::PROMPT_RENDERER_VERSION,
            input_fields: vec![InputFieldDefinition {
                key: "topic".into(),
                kind: InputFieldKind::Text {
                    min_bytes: 1,
                    max_bytes: 100,
                },
                required: true,
                default: Some(json!("the issue")),
                editable: true,
            }],
        },
        session: TriggerSessionPolicy {
            required_capabilities: vec![
                RequiredCapability::Conversation,
                RequiredCapability::DurableConversation,
                RequiredCapability::WorkspaceCheckpoint,
                RequiredCapability::RestrictedServiceGateway,
                RequiredCapability::CredentialRefresh,
                RequiredCapability::CredentialRevocation,
            ],
            permission_ceiling: PermissionMode::Autonomous,
            sandbox_profile: "cma-trigger-v1".into(),
            resources: ResourcePolicy {
                default: SessionSize::new(500, 1_024).unwrap(),
                ceiling: SessionSize::new(1_000, 2_048).unwrap(),
            },
            timeout_default_seconds: 3_600,
            timeout_ceiling_seconds: 7_200,
        },
        repository_access: RepositoryAccessPolicy {
            credential_route: CredentialRoute::ContributorConnection,
            required_scope_classes: vec![ScopeClass::PublicRepository],
            allowed_operation_classes: vec![RepositoryOperationClass::RepositoryRead],
            workflow_change_policy: WorkflowChangePolicy::Refuse,
        },
        setup: SetupPolicy {
            executor_policy: ExecutorPolicy::Disabled,
            steps: Vec::new(),
        },
        contribution: ContributionPolicy {
            branch_prefix: "cma/".into(),
            commit_attribution: CommitAttributionPolicy::ContributorNoReply,
            pull_request: PullRequestDefaults {
                title_template: "CMA contribution".into(),
                body_template: "Created by CMA".into(),
            },
        },
        commercial_policy: CommercialPolicy {
            billing_mode: BillingMode::ZeroCost,
            policy_reference: layer2_core::CMA_TRIGGER_ZERO_COST_POLICY_V1.into(),
        },
        override_policy: OverridePolicy {
            editable_fields: vec![EditableField::TaskInput],
            optional_checks: Vec::new(),
        },
        public_projection: PublicProjectionPolicy { version: 1 },
    }
}

fn upstream() -> RepositoryIdentity {
    RepositoryIdentity {
        repository_id: 10,
        repository_node_id: "R_upstream".into(),
        owner_id: 20,
        owner_node_id: "U_owner".into(),
        canonical_owner: "upstream".into(),
        canonical_name: "project".into(),
        visibility: RepositoryVisibility::Public,
        allow_forking: true,
        default_branch: "main".into(),
        selected_ref: "main".into(),
        fork_parent_id: None,
        fork_source_id: None,
    }
}

fn connection() -> ConnectionSnapshot {
    ConnectionSnapshot {
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "key-1".into(),
        account: ConnectedAccount {
            account_id: 21,
            account_node_id: "U_contributor".into(),
            login: "contributor".into(),
            avatar_url: None,
        },
        state: ConnectionState::Active,
        granted_scopes: vec!["public_repo".into()],
        satisfied_scope_classes: vec![ScopeClass::PublicRepository],
        consent_observed_at: NOW.into(),
        preflight_observed_at: NOW.into(),
    }
}

fn provenance(subject: SubjectId) -> AuthorityProvenance {
    AuthorityProvenance {
        subject,
        subject_token_generation: 1,
        service_id: "service-1".into(),
        service_state_version: 2,
        backing_key_id: "key-1".into(),
        session_credential_authority: None,
        state: DelegationState::Available,
        observed_at: NOW.into(),
    }
}
