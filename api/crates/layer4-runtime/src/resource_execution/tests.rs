use super::*;
use futures_util::future::BoxFuture;
use layer3_adapter::{
    owner_protocol::{OwnerConfirmation, TransportOwner},
    ConversationInput, ResourceInput,
};
use runner::{Cell, Quiescent, Termination};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tokio::sync::Notify;

fn id(value: &str) -> OpaqueId {
    OpaqueId::new(value).unwrap()
}
fn digest(byte: char) -> Digest {
    Digest::new(&byte.to_string().repeat(64)).unwrap()
}
fn generation(value: u64) -> Generation {
    Generation::new(value).unwrap()
}

struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn tick_ms(&self) -> ProtocolResult<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
impl TestClock {
    fn set(&self, value: u64) {
        self.0.store(value, Ordering::SeqCst);
    }
}

// Exercises receiver ordering only. This runner advertises no production or OS evidence.
struct TestRunner {
    entries: AtomicUsize,
    cancelled: AtomicBool,
    forced: AtomicUsize,
    force_fails: AtomicBool,
    hold_prepare: AtomicBool,
    preparing: Notify,
    release: Notify,
    quiescent: AtomicBool,
    reports_no_entry: AtomicBool,
    recovery_fails: AtomicBool,
    recoveries: AtomicUsize,
}
impl TestRunner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            forced: AtomicUsize::new(0),
            force_fails: AtomicBool::new(false),
            hold_prepare: AtomicBool::new(false),
            preparing: Notify::new(),
            release: Notify::new(),
            quiescent: AtomicBool::new(true),
            reports_no_entry: AtomicBool::new(false),
            recovery_fails: AtomicBool::new(false),
            recoveries: AtomicUsize::new(0),
        })
    }
}
impl SharedExecutionRunner for TestRunner {
    fn capabilities(&self) -> Option<ExecutionCapabilities> {
        Some(ExecutionCapabilities {
            protocol: 1,
            targeted_native_stop: true,
            mediated_effects: true,
            mixed_principal_control: false,
        })
    }
    fn prepare<'a>(&'a self, _: &'a Cell, _: ResourceInput) -> BoxFuture<'a, ProtocolResult<()>> {
        Box::pin(async move {
            self.preparing.notify_one();
            if self.hold_prepare.load(Ordering::SeqCst) {
                self.release.notified().await;
            }
            Ok(())
        })
    }
    fn enter(&self, _: &Cell, _: gate::EnterPermit<'_>) -> ProtocolResult<()> {
        self.entries.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn cancel(&self, _: &Cell, kind: Termination) -> ProtocolResult<()> {
        self.cancelled.store(true, Ordering::SeqCst);
        if matches!(kind, Termination::Force) {
            self.forced.fetch_add(1, Ordering::SeqCst);
            if self.force_fails.load(Ordering::SeqCst) {
                return Err(ProtocolError::Unknown);
            }
        }
        Ok(())
    }
    fn observe_quiescence<'a>(&'a self, _: &'a Cell) -> BoxFuture<'a, ProtocolResult<Quiescent>> {
        Box::pin(async move {
            if !self.quiescent.load(Ordering::SeqCst) {
                return Err(ProtocolError::Unknown);
            }
            Ok(Quiescent {
                evidence: if self.reports_no_entry.load(Ordering::SeqCst) {
                    StopEvidenceKind::RejectedBeforeStart
                } else {
                    StopEvidenceKind::CooperativeQuiescence
                },
                state: QuiescenceV1 {
                    queues_purged: true,
                    descendants_absent: true,
                    output_closed: true,
                    submitters_closed: true,
                    pending_local_effects_settled: true,
                    local_changes_digest: digest('c'),
                    effects: EffectJournalV1 {
                        high_water: 1,
                        digest: digest('d'),
                        unresolved_count: 1,
                    },
                },
            })
        })
    }
    fn recover<'a>(&'a self, _: &'a Cell) -> BoxFuture<'a, ProtocolResult<()>> {
        Box::pin(async {
            self.recoveries.fetch_add(1, Ordering::SeqCst);
            if self.recovery_fails.load(Ordering::SeqCst) {
                Err(ProtocolError::Unknown)
            } else {
                Ok(())
            }
        })
    }
}

struct Fixture {
    receiver: Arc<Receiver>,
    runner: Arc<TestRunner>,
    clock: Arc<TestClock>,
    owner: TransportOwner,
    authority: crate::manager_ownership::OwnerSession,
    root: PathBuf,
    _budget_fixture: tokio::sync::SemaphorePermit<'static>,
}
impl Fixture {
    fn restart(&mut self, enabled: bool) {
        let mut identity = self.receiver.identity.clone();
        identity.boot = id("boot-two");
        let placeholder = Receiver::with_runner(
            self.root.join("unactivated"),
            identity.clone(),
            false,
            self.runner.clone(),
            self.clock.clone(),
        )
        .unwrap();
        drop(std::mem::replace(&mut self.receiver, placeholder));
        self.receiver = Receiver::with_runner(
            self.root.join("receiver"),
            identity,
            enabled,
            self.runner.clone(),
            self.clock.clone(),
        )
        .unwrap();
    }

    async fn new() -> Self {
        // Each activated receiver reserves 16 MiB from the real process pool.
        // Bound simultaneous fixtures just as production bounds receivers.
        static FIXTURES: Semaphore = Semaphore::const_new(4);
        let budget_fixture = FIXTURES.acquire().await.unwrap();
        let root =
            std::env::temp_dir().join(format!("cma-resource-{}", uuid::Uuid::new_v4().simple()));
        let identity = ReceiverKey {
            kind: ReceiverKind::Runtime,
            instance: id("runtime"),
            incarnation: digest('a'),
            boot: id("boot-one"),
        };
        let snapshot = Snapshot {
            version: 1,
            assignment: Assignment {
                instance: identity.instance.clone(),
                incarnation: identity.incarnation.clone(),
            },
            policy_epoch: generation(1),
            sequence: generation(1),
            records: vec![],
            floors: vec![],
        };
        Journal::activate_fixture(&root.join("receiver"), &snapshot);
        let runner = TestRunner::new();
        let clock = Arc::new(TestClock(AtomicU64::new(100)));
        let receiver = Receiver::with_runner(
            root.join("receiver"),
            identity,
            true,
            runner.clone(),
            clock.clone(),
        )
        .unwrap();
        let owner = TransportOwner {
            protocol: 1,
            agent_id: "agent".into(),
            instance_id: "manager".into(),
            epoch: 1,
        };
        let owners = crate::manager_ownership::ManagerOwnership::new(root.join("owners"));
        let challenge = owners.challenge(owner.clone()).await.unwrap();
        owners
            .confirm(OwnerConfirmation {
                owner: owner.clone(),
                nonce: challenge.nonce,
            })
            .await
            .unwrap();
        let authority = owners.session("agent", Some(owner.clone())).await.unwrap();
        Self {
            receiver,
            runner,
            clock,
            owner,
            authority,
            root,
            _budget_fixture: budget_fixture,
        }
    }
    async fn dispatch(&self, command: &str) -> layer3_adapter::ResourceDispatch {
        let execution = ExecutionKey {
            command_id: id(command),
            command_fingerprint: digest('b'),
            dispatch_sequence: generation(1),
            prompt_id: id("prompt"),
            turn_id: id("turn"),
            agent_id: id("agent"),
            receiver: self.receiver.identity.clone(),
            attempt: generation(1),
        };
        let challenge = self
            .receiver
            .challenge(self.owner.clone(), execution.clone())
            .await
            .unwrap();
        let input = ResourceInput::admit(ConversationInput::Prompt(vec![])).unwrap();
        input
            .clone()
            .bind(ExecutionTicketV1 {
                protocol: 1,
                execution,
                basis: ExecutionBasisV1::Share {
                    grant: GrantKey {
                        family: GrantFamilyKey {
                            origin: digest('1'),
                            share_id: id("share"),
                            generation: generation(1),
                        },
                        share_revision: generation(1),
                        target_ancestry: digest('2'),
                        capability: ProtectedCapability::Prompt,
                        policy_revision: generation(1),
                        actor: digest('3'),
                    },
                },
                operation: ExecutionOperation::Prompt,
                input_digest: input.digest().clone(),
                manager: ManagerKey {
                    instance: id("manager"),
                    epoch: generation(1),
                    agent_id: id("agent"),
                },
                challenge,
                nonce: id(command),
                registration: digest('4'),
            })
            .unwrap()
    }
    fn barrier(&self, dispatch: &layer3_adapter::ResourceDispatch) -> BarrierRequestV1 {
        BarrierRequestV1 {
            protocol: 1,
            revocation: RevocationKeyV1 {
                operation_id: digest('5'),
                family: dispatch.ticket().basis.family().unwrap().clone(),
                watermark: generation(2),
            },
            member: MemberKey::Execution {
                execution: dispatch.ticket().execution.clone(),
            },
            registration: dispatch.ticket().registration.clone(),
            reason: StopReason::ShareRevoked,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn barrier_before_start_returns_real_no_entry_receipt_and_is_replayable() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    f.receiver
        .barrier(f.owner.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        f.receiver
            .start(f.authority.clone(), dispatch)
            .await
            .unwrap_err(),
        ProtocolError::Expired
    );
    let receipt = f
        .receiver
        .stop(f.owner.clone(), request.clone(), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.evidence, StopEvidenceKind::RejectedBeforeStart);
    assert_eq!(receipt.quiescence.effects.high_water, 0);
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
    assert_eq!(
        Some(receipt),
        f.receiver
            .stop(f.owner.clone(), request, false)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn barrier_during_pending_prepare_prevents_enter_and_waits_for_prepare_owner() {
    let f = Fixture::new().await;
    f.runner.hold_prepare.store(true, Ordering::SeqCst);
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    let receiver = f.receiver.clone();
    let authority = f.authority.clone();
    let start = tokio::spawn(async move { receiver.start(authority, dispatch).await });
    f.runner.preparing.notified().await;
    f.receiver
        .barrier(f.owner.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap_err(),
        ProtocolError::Unknown
    );
    f.runner.release.notify_one();
    assert_eq!(start.await.unwrap().unwrap_err(), ProtocolError::Expired);
    let receipt = f
        .receiver
        .stop(f.owner.clone(), request, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.evidence, StopEvidenceKind::RejectedBeforeStart);
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn duplicate_start_recovers_evidence_without_executing_again() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let first = f
        .receiver
        .start(f.authority.clone(), dispatch.clone())
        .await
        .unwrap();
    f.clock.set(5000);
    let replay = f
        .receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stop_needs_observed_quiescence_and_preserves_unknown_remote_effects() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.runner.quiescent.store(false, Ordering::SeqCst);
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap_err(),
        ProtocolError::Unknown
    );
    assert!(f
        .receiver
        .stop(f.owner.clone(), request.clone(), false)
        .await
        .unwrap()
        .is_none());
    f.runner.quiescent.store(true, Ordering::SeqCst);
    let receipt = f
        .receiver
        .stop(f.owner.clone(), request, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.evidence, StopEvidenceKind::CooperativeQuiescence);
    assert_eq!(receipt.quiescence.effects.unresolved_count, 1);
}

#[tokio::test]
async fn delayed_continuation_does_not_extend_its_original_challenge() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let ticket = dispatch.ticket().clone();
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.clock.set(1100);
    let challenge = f
        .receiver
        .challenge(f.owner.clone(), ticket.execution.clone())
        .await
        .unwrap();
    f.clock.set(3000);
    let lease = ContinuationV1 {
        protocol: 1,
        execution: ticket.execution,
        registration: ticket.registration,
        basis: ticket.basis,
        manager: ticket.manager,
        challenge,
        sequence: generation(2),
    };
    f.receiver
        .continue_execution(f.authority.clone(), lease.clone())
        .await
        .unwrap();
    f.clock.set(4100);
    assert_eq!(
        f.receiver
            .continue_execution(f.authority.clone(), lease)
            .await
            .unwrap_err(),
        ProtocolError::Expired
    );
}

#[tokio::test]
async fn queued_start_cannot_renew_and_wrong_successor_cannot_refresh_running_work() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let ticket = dispatch.ticket().clone();
    f.clock.set(1100);
    assert_eq!(
        f.receiver
            .challenge(f.owner.clone(), ticket.execution.clone())
            .await
            .unwrap(),
        ticket.challenge
    );
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    let challenge = f
        .receiver
        .challenge(f.owner.clone(), ticket.execution.clone())
        .await
        .unwrap();
    let lease = ContinuationV1 {
        protocol: 1,
        execution: ticket.execution,
        registration: ticket.registration,
        basis: ticket.basis,
        manager: ticket.manager,
        challenge,
        sequence: generation(3),
    };
    assert_eq!(
        f.receiver
            .continue_execution(f.authority.clone(), lease)
            .await
            .unwrap_err(),
        ProtocolError::EvidenceMismatch
    );
}

#[tokio::test]
async fn watchdog_closes_and_escalates_while_the_journal_lock_is_stalled() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    let gate = f.receiver.state.lock().unwrap().gates[0].clone();
    let _journal = f.receiver.state.lock().unwrap();
    f.clock.set(3100);
    wait_thread(|| gate.ready_to_observe_stop());
    f.clock.set(5100);
    wait_thread(|| f.runner.forced.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn independently_authorized_owner_does_not_acquire_shared_three_second_timeout() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("owner").await;
    let mut ticket = dispatch.ticket().clone();
    ticket.basis = ExecutionBasisV1::IndependentOwner {
        owner: digest('6'),
        actor: digest('7'),
        target_ancestry: digest('8'),
        capability: ProtectedCapability::Prompt,
    };
    let dispatch = dispatch.input().clone().bind(ticket).unwrap();
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.clock.set(30_000);
    std::thread::sleep(std::time::Duration::from_millis(75));
    assert!(!f.runner.cancelled.load(Ordering::SeqCst));
}

#[test]
fn semantic_flag_is_strict_and_never_activated_startup_writes_nothing() {
    for value in [None, Some("false")] {
        assert!(!enabled(value).unwrap());
    }
    assert!(enabled(Some("true")).unwrap());
    for value in [Some("1"), Some("TRUE"), Some(""), Some(" true")] {
        assert!(enabled(value).is_err());
    }
    let root = std::env::temp_dir().join(format!("cma-unactivated-{}", uuid::Uuid::new_v4()));
    let identity = ReceiverKey {
        kind: ReceiverKind::Runtime,
        instance: id("runtime"),
        incarnation: digest('a'),
        boot: id("boot"),
    };
    let receiver = Receiver::with_runner(
        root.clone(),
        identity,
        false,
        Arc::new(UnavailableRunner),
        Arc::new(BootClock),
    )
    .unwrap();
    assert!(receiver.legacy_allowed());
    assert!(receiver.capabilities().is_none());
    assert!(!root.exists());
    assert!(!root.with_extension("activated").exists());
}

#[tokio::test]
async fn activated_corruption_or_a_second_writer_never_selects_legacy() {
    let f = Fixture::new().await;
    let root = f.root.join("receiver");
    assert!(Receiver::with_runner(
        root.clone(),
        f.receiver.identity.clone(),
        false,
        f.runner.clone(),
        f.clock.clone()
    )
    .is_err());
    std::fs::remove_file(root.join("state.json")).unwrap();
    assert!(f.receiver.state().is_err());
    assert!(!f.receiver.legacy_allowed());
    assert!(Receiver::with_runner(
        root,
        f.receiver.identity.clone(),
        false,
        f.runner.clone(),
        f.clock.clone()
    )
    .is_err());
}

fn wait_thread(ready: impl Fn() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !ready() {
        assert!(
            std::time::Instant::now() < until,
            "independent watchdog did not progress"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[path = "journal_custody_tests.rs"]
mod journal_custody_tests;
#[path = "race_tests.rs"]
mod race_tests;
#[path = "restart_tests.rs"]
mod restart_tests;
