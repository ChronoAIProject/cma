use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use layer2_core::{
    seconds_from_rfc3339, BranchRecord, CapabilityRevocationState, FailureClass, Launch,
    LaunchFailure, LaunchState, ReachStatus, SessionId, SessionLink, TriggerService,
};

use super::super::worker::tests::{
    launch_fixture, rollout_policy, service_for_launch, service_for_launch_with_rollout,
    RecordingActivityStore,
};
use super::*;

const NOW: &str = "2026-08-27T00:00:00Z";
const ONE_SECOND_LATER: &str = "2026-08-27T00:00:01Z";

struct TestClock {
    times: Mutex<VecDeque<SystemTime>>,
}

impl CapabilityClock for TestClock {
    fn now(&self) -> SystemTime {
        self.times
            .lock()
            .unwrap()
            .pop_front()
            .expect("the capability worker sampled only the declared test times")
    }
}

fn test_worker(
    service: Arc<TriggerService>,
    executor: Arc<ScriptedExecutor>,
    holder: &str,
    times: &[&str],
) -> CapabilityWorker {
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
    CapabilityWorker::with_clock(service, executor, holder, clock).unwrap()
}

#[derive(Debug)]
enum RenewalStep {
    Succeed {
        runtime_audience: String,
        branch_head: String,
        expires_at_epoch_seconds: u64,
    },
    Panic,
}

#[derive(Debug)]
enum RevocationStep {
    Acknowledge {
        runtime_audience: String,
        tombstone: bool,
        agent_key: bool,
    },
    Fail(LaunchFailure),
}

struct ScriptedExecutor {
    runtime_answers: Mutex<VecDeque<Result<String, LaunchFailure>>>,
    renewal_steps: Mutex<VecDeque<RenewalStep>>,
    revocation_steps: Mutex<VecDeque<RevocationStep>>,
    runtime_calls: AtomicUsize,
    renewal_calls: Mutex<Vec<(u64, u64)>>,
    revocation_calls: AtomicUsize,
}

impl ScriptedExecutor {
    fn new(
        runtime_answers: Vec<Result<String, LaunchFailure>>,
        renewal_steps: Vec<RenewalStep>,
        revocation_steps: Vec<RevocationStep>,
    ) -> Self {
        Self {
            runtime_answers: Mutex::new(runtime_answers.into()),
            renewal_steps: Mutex::new(renewal_steps.into()),
            revocation_steps: Mutex::new(revocation_steps.into()),
            runtime_calls: AtomicUsize::new(0),
            renewal_calls: Mutex::new(Vec::new()),
            revocation_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl CapabilityExecutor for ScriptedExecutor {
    async fn runtime_audience(
        &self,
        _launch: &Launch,
        _fence: u64,
    ) -> Result<String, LaunchFailure> {
        self.runtime_calls.fetch_add(1, Ordering::SeqCst);
        self.runtime_answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("the test configured every runtime observation")
    }

    async fn renew(&self, launch: &Launch, fence: u64) -> Result<CapabilityRenewal, LaunchFailure> {
        let generation = launch.reach.as_ref().unwrap().policy_generation + 1;
        self.renewal_calls.lock().unwrap().push((generation, fence));
        let step = self
            .renewal_steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("the test configured every renewal attempt");
        match step {
            RenewalStep::Succeed {
                runtime_audience,
                branch_head,
                expires_at_epoch_seconds,
            } => {
                let mut branch = launch.branch.clone().unwrap();
                branch.current_head = branch_head;
                Ok(CapabilityRenewal {
                    reach: active_reach(
                        generation,
                        fence,
                        &runtime_audience,
                        expires_at_epoch_seconds,
                    ),
                    connection: launch.connection.clone().unwrap(),
                    authority: launch.authority.clone().unwrap(),
                    branch,
                })
            }
            RenewalStep::Panic => panic!("simulated crash after runtime policy installation"),
        }
    }

    async fn revoke(
        &self,
        launch: &Launch,
        fence: u64,
    ) -> Result<CapabilityRevocation, LaunchFailure> {
        self.revocation_calls.fetch_add(1, Ordering::SeqCst);
        match self
            .revocation_steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("the test configured every revocation attempt")
        {
            RevocationStep::Acknowledge {
                runtime_audience,
                tombstone,
                agent_key,
            } => Ok(CapabilityRevocation {
                generation: launch.reach.as_ref().unwrap().policy_generation + 1,
                fence,
                runtime_audience,
                tombstone_acknowledged: tombstone,
                agent_key_revocation_confirmed: agent_key,
            }),
            RevocationStep::Fail(failure) => Err(failure),
        }
    }
}

fn epoch(value: &str) -> u64 {
    seconds_from_rfc3339(value).unwrap()
}

fn active_reach(
    generation: u64,
    fence: u64,
    runtime_audience: &str,
    expires_at_epoch_seconds: u64,
) -> ReachStatus {
    ReachStatus {
        service_id: "service-1".into(),
        state: CapabilityRevocationState::Active,
        policy_generation: generation,
        policy_fence: fence,
        runtime_audience: runtime_audience.into(),
        expires_at_epoch_seconds,
        numeric_binding_observed_at: NOW.into(),
        refreshed_at: NOW.into(),
        revocation_requested_at: None,
        revocation_effective_at: None,
        last_refusal: None,
        tombstone_acknowledged: false,
        agent_key_revocation_confirmed: false,
    }
}

fn launch_with_active_reach(expires_at_epoch_seconds: u64) -> Launch {
    let mut launch = launch_fixture();
    launch.state = LaunchState::Ready;
    launch.fencing_counter = 7;
    launch.branch = Some(BranchRecord {
        name: format!("cma/{}", launch.id.0),
        expected_head: "a".repeat(40),
        current_head: "a".repeat(40),
        canonical_url: format!(
            "https://github.com/contributor/project/tree/cma/{}",
            launch.id.0
        ),
        created_at: NOW.into(),
    });
    launch.session = Some(SessionLink {
        session_id: SessionId(format!("sesn_{}", launch.id.0)),
        environment_profile_id: "trigger-profile".into(),
        runtime_generation: 1,
        created_at: NOW.into(),
    });
    launch.reach = Some(active_reach(
        3,
        launch.fencing_counter,
        "runtime-v1",
        expires_at_epoch_seconds,
    ));
    launch
}

fn launch_with_pending_revocation() -> Launch {
    let mut launch = launch_with_active_reach(epoch(NOW) + 60);
    launch.state = LaunchState::Cancelled;
    let reach = launch.reach.as_mut().unwrap();
    reach.state = CapabilityRevocationState::RevocationPending;
    reach.revocation_requested_at = Some(NOW.into());
    launch
}

fn successful_renewal(runtime_audience: &str, branch_head: &str) -> RenewalStep {
    RenewalStep::Succeed {
        runtime_audience: runtime_audience.into(),
        branch_head: branch_head.into(),
        expires_at_epoch_seconds: epoch(NOW) + 600,
    }
}

fn retryable_failure(code: &str) -> LaunchFailure {
    LaunchFailure {
        class: FailureClass::RetryableTransient,
        code: code.into(),
        detail: "The maintenance boundary is temporarily unavailable.".into(),
        remediation: "CMA will retry from durable state.".into(),
        correlation_id: "capability-test".into(),
        retry_at: None,
        observed_at: NOW.into(),
    }
}

async fn stored(service: &TriggerService, launch: &Launch) -> Launch {
    service.get_launch(&launch.owner, &launch.id).await.unwrap()
}

#[tokio::test]
async fn renews_inside_the_expiry_margin_without_an_extra_runtime_probe() {
    let launch = launch_with_active_reach(epoch(NOW) + 20);
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        vec![successful_renewal("runtime-v1", &"a".repeat(40))],
        Vec::new(),
    ));
    let worker = test_worker(
        service.clone(),
        executor.clone(),
        "renewal-worker",
        &[NOW, ONE_SECOND_LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "renew-before-expiry")
            .await
            .unwrap(),
        CapabilityRunOutcome::Renewed
    );
    assert_eq!(executor.runtime_calls.load(Ordering::SeqCst), 0);
    let current = stored(&service, &launch).await;
    assert_eq!(current.reach.unwrap().policy_generation, 4);
    assert!(current.lease.is_none());
}

#[tokio::test]
async fn capability_maintenance_audit_is_closed_and_never_retains_authority_values() {
    let activity_store = Arc::new(RecordingActivityStore::default());
    let activity = layer2_core::ActivityService::new(activity_store.clone());

    let renewal_launch = launch_with_active_reach(epoch(NOW) + 20);
    let renewal_service = service_for_launch(renewal_launch.clone());
    let renewal_executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        vec![successful_renewal("runtime-v1", &"a".repeat(40))],
        Vec::new(),
    ));
    let renewal_worker = test_worker(
        renewal_service,
        renewal_executor,
        "renewal-audit-worker",
        &[NOW, ONE_SECOND_LATER],
    )
    .with_activity(Arc::clone(&activity));
    assert_eq!(
        renewal_worker
            .run_once(
                &renewal_launch.id,
                renewal_launch.revision_number,
                "renewal-audit",
            )
            .await
            .unwrap(),
        CapabilityRunOutcome::Renewed
    );

    let revocation_launch = launch_with_pending_revocation();
    let revocation_service = service_for_launch(revocation_launch.clone());
    let revocation_executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        Vec::new(),
        vec![RevocationStep::Acknowledge {
            runtime_audience: "runtime-v1".into(),
            tombstone: true,
            agent_key: false,
        }],
    ));
    let revocation_worker = test_worker(
        revocation_service,
        revocation_executor,
        "revocation-audit-worker",
        &[NOW, ONE_SECOND_LATER],
    )
    .with_activity(activity);
    assert_eq!(
        revocation_worker
            .run_once(
                &revocation_launch.id,
                revocation_launch.revision_number,
                "revocation-audit",
            )
            .await
            .unwrap(),
        CapabilityRunOutcome::Revoked
    );

    let records = activity_store.records.lock().await;
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].action,
        layer2_core::ActivityAction::TriggerCapabilityRenew
    );
    assert_eq!(
        records[1].action,
        layer2_core::ActivityAction::TriggerCapabilityRevoke
    );
    for record in records.iter() {
        assert_eq!(
            record.detail.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            vec![
                layer2_core::DetailKey::Subject,
                layer2_core::DetailKey::CorrelationId,
                layer2_core::DetailKey::RevisionDigest,
                layer2_core::DetailKey::RepositoryId,
                layer2_core::DetailKey::GithubAccountId,
                layer2_core::DetailKey::ServiceId,
                layer2_core::DetailKey::LaunchState,
                layer2_core::DetailKey::FencingToken,
                layer2_core::DetailKey::PolicyGeneration,
            ]
        );
        let wire = serde_json::to_string(record).unwrap();
        for forbidden in [
            "credential",
            "access_token",
            "github_token",
            "signed_policy",
            "provider_body",
            "repository_path",
        ] {
            assert!(
                !wire.contains(forbidden),
                "capability audit admitted {forbidden}: {wire}"
            );
        }
    }
}

#[tokio::test]
async fn runtime_rotation_forces_immediate_renewal() {
    let launch = launch_with_active_reach(epoch(NOW) + 60);
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        vec![Ok("runtime-v2".into())],
        vec![successful_renewal("runtime-v2", &"a".repeat(40))],
        Vec::new(),
    ));
    let worker = test_worker(
        service.clone(),
        executor.clone(),
        "rotation-worker",
        &[NOW, ONE_SECOND_LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "runtime-rotated")
            .await
            .unwrap(),
        CapabilityRunOutcome::Renewed
    );
    assert_eq!(executor.runtime_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        stored(&service, &launch)
            .await
            .reach
            .unwrap()
            .runtime_audience,
        "runtime-v2"
    );
}

#[tokio::test]
async fn renewal_persists_the_current_launch_branch_head() {
    let launch = launch_with_active_reach(epoch(NOW) + 20);
    let service = service_for_launch(launch.clone());
    let refreshed_head = "b".repeat(40);
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        vec![successful_renewal("runtime-v1", &refreshed_head)],
        Vec::new(),
    ));
    let worker = test_worker(
        service.clone(),
        executor,
        "branch-observer",
        &[NOW, ONE_SECOND_LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "branch-advanced")
            .await
            .unwrap(),
        CapabilityRunOutcome::Renewed
    );
    assert_eq!(
        stored(&service, &launch).await.branch.unwrap().current_head,
        refreshed_head
    );
}

#[tokio::test]
async fn crash_repeats_the_generation_under_a_higher_worker_fence() {
    let launch = launch_with_active_reach(epoch(NOW) + 20);
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        vec![
            RenewalStep::Panic,
            successful_renewal("runtime-v1", &"a".repeat(40)),
        ],
        Vec::new(),
    ));
    let crashed_worker = test_worker(service.clone(), executor.clone(), "crashed-worker", &[NOW]);
    let crashed_id = launch.id.clone();
    let crash = tokio::spawn(async move {
        crashed_worker
            .run_once(&crashed_id, 0, "crash-after-install")
            .await
    })
    .await
    .expect_err("the simulated worker crash must strand its lease");
    assert!(crash.is_panic());

    let stranded = stored(&service, &launch).await;
    assert_eq!(stranded.reach.as_ref().unwrap().policy_generation, 3);
    assert!(stranded.lease.is_some());
    let replacement = test_worker(
        service.clone(),
        executor.clone(),
        "replacement-worker",
        &["2026-08-27T00:02:01Z", "2026-08-27T00:02:02Z"],
    );
    assert_eq!(
        replacement
            .run_once(&launch.id, stranded.revision_number, "take-over-renewal")
            .await
            .unwrap(),
        CapabilityRunOutcome::Renewed
    );
    assert_eq!(
        executor.renewal_calls.lock().unwrap().as_slice(),
        &[(4, 8), (4, 9)]
    );
}

#[tokio::test]
async fn tombstone_acknowledgement_makes_revocation_effective() {
    let launch = launch_with_pending_revocation();
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        Vec::new(),
        vec![RevocationStep::Acknowledge {
            runtime_audience: "runtime-v1".into(),
            tombstone: true,
            agent_key: false,
        }],
    ));
    let worker = test_worker(
        service.clone(),
        executor.clone(),
        "revocation-worker",
        &[NOW, ONE_SECOND_LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "tombstone-ack")
            .await
            .unwrap(),
        CapabilityRunOutcome::Revoked
    );
    let current = stored(&service, &launch).await;
    let reach = current.reach.as_ref().unwrap();
    assert_eq!(reach.state, CapabilityRevocationState::Revoked);
    assert!(reach.tombstone_acknowledged);
    assert!(!reach.agent_key_revocation_confirmed);
    assert!(!current.capability_requires_revocation());
}

#[tokio::test]
async fn provider_key_settlement_makes_revocation_effective_without_a_runtime() {
    let launch = launch_with_pending_revocation();
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        Vec::new(),
        vec![RevocationStep::Acknowledge {
            runtime_audience: "disconnected-runtime".into(),
            tombstone: false,
            agent_key: true,
        }],
    ));
    let worker = test_worker(
        service.clone(),
        executor,
        "provider-revocation-worker",
        &[NOW, ONE_SECOND_LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "provider-key-ack")
            .await
            .unwrap(),
        CapabilityRunOutcome::Revoked
    );
    let reach = stored(&service, &launch).await.reach.unwrap();
    assert_eq!(reach.state, CapabilityRevocationState::Revoked);
    assert!(!reach.tombstone_acknowledged);
    assert!(reach.agent_key_revocation_confirmed);
}

#[tokio::test]
async fn no_enforcement_acknowledgement_stays_revocation_pending() {
    let launch = launch_with_pending_revocation();
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        Vec::new(),
        vec![RevocationStep::Fail(retryable_failure(
            "trigger_revocation_pending",
        ))],
    ));
    let worker = test_worker(
        service.clone(),
        executor,
        "pending-revocation-worker",
        &[NOW],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "no-ack")
            .await
            .unwrap(),
        CapabilityRunOutcome::Deferred
    );
    let reach = stored(&service, &launch).await.reach.unwrap();
    assert_eq!(reach.state, CapabilityRevocationState::RevocationPending);
    assert!(reach.revocation_effective_at.is_none());
}

#[tokio::test]
async fn capability_kill_switch_persists_revocation_before_enforcement() {
    let launch = launch_with_active_reach(epoch(NOW) + 600);
    let original_session = launch.session.clone();
    let service =
        service_for_launch_with_rollout(launch.clone(), rollout_policy(&launch, false, true));
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        Vec::new(),
        vec![RevocationStep::Acknowledge {
            runtime_audience: "runtime-v1".into(),
            tombstone: true,
            agent_key: false,
        }],
    ));
    let worker = test_worker(
        service.clone(),
        executor.clone(),
        "kill-switch-worker",
        &[NOW, ONE_SECOND_LATER],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "kill-switch")
            .await
            .unwrap(),
        CapabilityRunOutcome::Revoked
    );
    assert_eq!(executor.revocation_calls.load(Ordering::SeqCst), 1);
    assert!(executor.renewal_calls.lock().unwrap().is_empty());
    let current = stored(&service, &launch).await;
    assert_eq!(current.state, LaunchState::Ready);
    assert_eq!(current.session, original_session);
    let reach = current.reach.unwrap();
    assert_eq!(reach.state, CapabilityRevocationState::Revoked);
    assert_eq!(reach.revocation_requested_at.as_deref(), Some(NOW));
    assert_eq!(
        reach.revocation_effective_at.as_deref(),
        Some(ONE_SECOND_LATER)
    );
}

#[tokio::test]
async fn failed_kill_switch_enforcement_remains_durably_revocation_pending() {
    let launch = launch_with_active_reach(epoch(NOW) + 600);
    let original_session = launch.session.clone();
    let service =
        service_for_launch_with_rollout(launch.clone(), rollout_policy(&launch, false, true));
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        Vec::new(),
        vec![RevocationStep::Fail(retryable_failure(
            "trigger_revocation_pending",
        ))],
    ));
    let worker = test_worker(
        service.clone(),
        executor,
        "failed-kill-switch-worker",
        &[NOW],
    );

    assert_eq!(
        worker
            .run_once(&launch.id, launch.revision_number, "kill-switch-no-ack")
            .await
            .unwrap(),
        CapabilityRunOutcome::Deferred
    );
    let current = stored(&service, &launch).await;
    assert_eq!(current.state, LaunchState::Ready);
    assert_eq!(current.session, original_session);
    let reach = current.reach.unwrap();
    assert_eq!(reach.state, CapabilityRevocationState::RevocationPending);
    assert_eq!(reach.revocation_requested_at.as_deref(), Some(NOW));
    assert!(reach.revocation_effective_at.is_none());
}

#[tokio::test]
async fn an_expired_lease_cannot_commit_a_maintenance_result() {
    let launch = launch_with_active_reach(epoch(NOW) + 20);
    let service = service_for_launch(launch.clone());
    let executor = Arc::new(ScriptedExecutor::new(
        Vec::new(),
        vec![successful_renewal("runtime-v1", &"a".repeat(40))],
        Vec::new(),
    ));
    let worker = test_worker(
        service.clone(),
        executor,
        "slow-renewal-worker",
        &[NOW, "2026-08-27T00:02:01Z"],
    );

    assert!(matches!(
        worker
            .run_once(&launch.id, launch.revision_number, "lease-expired")
            .await,
        Err(TriggerServiceError::Validation(_))
    ));
    let stranded = stored(&service, &launch).await;
    assert_eq!(stranded.reach.unwrap().policy_generation, 3);
    assert!(stranded.lease.is_some());
}
