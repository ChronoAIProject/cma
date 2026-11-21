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
#[path = "state_tests.rs"]
mod state_tests;
