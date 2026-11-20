use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
};
use crate::sandbox::{AgentLogEntry, AgentLogKind, AgentMessageAssembly};

struct Recorder {
    policy: IngressOwnershipPolicy,
    calls: AtomicUsize,
    fail: bool,
    block: bool,
}

fn fixture() -> (IngressBudget, Recorder) {
    let root = IngressBudget::new(32768);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(32768, plan(1024), plan(8192)).unwrap(),
    )
    .unwrap();
    (
        root,
        Recorder {
            policy,
            calls: AtomicUsize::new(0),
            fail: false,
            block: false,
        },
    )
}

fn idle_bytes(root: &IngressBudget) -> usize {
    root.capacity_bytes() - root.metadata_bytes()
}

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("recorder operation fixture", &[bytes]).unwrap()
}

impl Recorder {
    fn operation<'a, T: Send + 'a>(
        &'a self,
        call: usize,
        initialize: impl FnOnce() -> T + Send + 'a,
    ) -> Result<AdmittedOperation<'a, Result<T, AgentConversationError>>, AdmittedError> {
        AdmittedOperation::from_owned(
            &self.policy,
            IngressClass::Transformation,
            plan(1024),
            move |_| async move {
                let retained =
                    IngressWorkScope::new(self.policy.admit(IngressClass::Data, plan(4096))?);
                let result = retained.initialize_owned(|| {
                    self.calls.fetch_or(call, Ordering::SeqCst);
                    if self.fail {
                        Err(AgentConversationError::Unavailable(
                            "retained error".repeat(64),
                        ))
                    } else {
                        Ok(initialize())
                    }
                });
                if self.block {
                    std::future::pending::<()>().await;
                }
                Ok(result)
            },
        )
    }
}

fn outcome(id: &AgentId, at: SystemTime) -> AgentUpdateOutcome {
    AgentUpdateOutcome {
        appended: vec![AgentLogEntry {
            agent_id: id.clone(),
            seq: 1,
            appended_at: rfc3339_at(at),
            kind: AgentLogKind::AgentMessage {
                content: "retained message".repeat(64),
            },
        }],
        turn_closed: false,
    }
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
    fn recording_session(&self) -> RecordingSession {
        RecordingSession::new(Some(self.policy.clone()))
    }

    fn restore_session_operation<'a>(
        &'a self,
        _: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
    ) -> Result<AdmittedOperation<'a, Result<(), AgentConversationError>>, AdmittedError> {
        self.operation(1, || ())
    }

    fn record_session_update_operation<'a>(
        &'a self,
        id: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
        _: &'a ConversationUpdate,
        at: SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentUpdateOutcome, AgentConversationError>>,
        AdmittedError,
    > {
        self.operation(2, move || outcome(id, at))
    }

    fn record_session_output_operation<'a>(
        &'a self,
        id: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
        _: &'a ConversationUpdate,
        at: SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentOutputCommit, AgentConversationError>>,
        AdmittedError,
    > {
        self.operation(4, move || AgentOutputCommit {
            outcome: outcome(id, at),
            committed: None,
        })
    }

    fn record_session_reopen_operation<'a>(
        &'a self,
        id: &'a AgentId,
        _: &'a AgentReopenGuard,
        _: &'a mut RecordingSession,
        _: &'a ConversationUpdate,
        at: SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentUpdateOutcome, AgentConversationError>>,
        AdmittedError,
    > {
        self.operation(8, move || outcome(id, at))
    }

    fn flush_session_output_operation<'a>(
        &'a self,
        _: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
        _: SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<Option<RuntimeOutputSource>, AgentConversationError>>,
        AdmittedError,
    > {
        self.operation(16, || None)
    }

    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("enabled entry must never invoke the legacy recorder")
    }
}

fn guard() -> AgentReopenGuard {
    AgentReopenGuard {
        sandbox: crate::sandbox::store::SandboxMutationGuard {
            sandbox_id: crate::SandboxId("sbx_operation_contract".into()),
            owner: crate::SubjectId::new("synthetic-owner"),
            expected_revision: 1,
            status: crate::sandbox::SandboxStatus::Reviving,
            work_step: crate::sandbox::SandboxWorkStep::RunLifecycleOperation,
            lease: None,
            operation: None,
            runtime: None,
            checkpoint_capture: None,
            background_checkpoint: None,
            checkpoint_epoch_high_water: 0,
        },
        agent_fence: 19,
        runtime_generation: 7,
        native_conversation_id: "synthetic-conversation".into(),
    }
}

#[tokio::test]
async fn enabled_helpers_select_every_admitted_port_and_keep_the_original_timestamp() {
    let (root, recorder) = fixture();
    let service: &dyn AgentUpdateRecorder = &recorder;
    let mut session = service.recording_session();
    let id = AgentId("agt_operation_contract".into());
    let update = ConversationUpdate::Lost { frames: 3 };
    let at = std::time::UNIX_EPOCH;
    let operation = service
        .restore_session_operation(&id, 7, &mut session)
        .unwrap();
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    drop(operation);
    assert_eq!(root.available_bytes(), idle_bytes(&root));
    service
        .restore_recording(&id, 7, &mut session)
        .await
        .unwrap();
    let result = service
        .record_update_in_session(&id, 7, &mut session, &update, at)
        .await
        .unwrap();
    assert_eq!(result.appended[0].appended_at, "1970-01-01T00:00:00Z");
    assert!(result.belongs_to(&recorder.policy));
    assert_eq!(root.available_bytes(), idle_bytes(&root) - 4096);
    drop(result);
    service
        .record_output_in_session(&id, 7, &mut session, &update, at)
        .await
        .unwrap();
    service
        .record_reopen_in_session(&id, &guard(), &mut session, &update, at)
        .await
        .unwrap();
    service
        .flush_recording(&id, 7, &mut session, at)
        .await
        .unwrap();
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 31);
    assert_eq!(root.available_bytes(), idle_bytes(&root));
}

#[tokio::test]
async fn operation_refusal_precedes_initialization_and_future_storage() {
    let (root, recorder) = fixture();
    let held = recorder
        .policy
        .admit(IngressClass::Transformation, plan(8192))
        .unwrap();
    let service: &dyn AgentUpdateRecorder = &recorder;
    let mut session = service.recording_session();
    let error = service
        .record_output_in_session(
            &AgentId("agt_refused_operation".into()),
            7,
            &mut session,
            &ConversationUpdate::Lost { frames: 3 },
            std::time::UNIX_EPOCH,
        )
        .await
        .unwrap_err();
    assert_eq!(*error, AgentConversationError::CapacityExceeded);
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), idle_bytes(&root) - 8192);
    drop(held);
    assert_eq!(root.available_bytes(), idle_bytes(&root));
}

#[tokio::test]
async fn domain_error_keeps_its_data_owner_after_operation_storage_is_gone() {
    let (root, mut recorder) = fixture();
    recorder.fail = true;
    let service: &dyn AgentUpdateRecorder = &recorder;
    let mut session = service.recording_session();
    let error = service
        .record_output_in_session(
            &AgentId("agt_operation_error".into()),
            7,
            &mut session,
            &ConversationUpdate::Lost { frames: 3 },
            std::time::UNIX_EPOCH,
        )
        .await
        .unwrap_err();
    assert_eq!(
        *error,
        AgentConversationError::Unavailable("retained error".repeat(64))
    );
    assert!(error.belongs_to(&recorder.policy));
    let available = recorder
        .policy
        .admit(IngressClass::Transformation, plan(8192))
        .unwrap();
    drop(available);
    assert_eq!(root.available_bytes(), idle_bytes(&root) - 4096);
    drop(error);
    assert_eq!(root.available_bytes(), idle_bytes(&root));
}

#[tokio::test]
async fn cancellation_drops_pending_result_and_operation_with_their_owners() {
    let (root, mut recorder) = fixture();
    recorder.block = true;
    let service: &dyn AgentUpdateRecorder = &recorder;
    let mut session = service.recording_session();
    let id = AgentId("agt_cancelled_operation".into());
    let update = ConversationUpdate::Lost { frames: 3 };
    let mut operation = Box::pin(service.record_output_in_session(
        &id,
        7,
        &mut session,
        &update,
        std::time::UNIX_EPOCH,
    ));
    assert!(futures_util::poll!(&mut operation).is_pending());
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 4);
    assert!(root.available_bytes() < idle_bytes(&root) - 4096);
    drop(operation);
    assert_eq!(root.available_bytes(), idle_bytes(&root));
}

#[tokio::test]
async fn operation_errors_are_classified_without_copying_diagnostics() {
    let (root, recorder) = fixture();
    for capacity in [false, true] {
        let retained = IngressWorkScope::new(
            recorder
                .policy
                .admit(IngressClass::Data, plan(4096))
                .unwrap(),
        );
        let error = retained.initialize(|| {
            if capacity {
                AdapterError::AtCapacity("owned diagnostic".repeat(128))
            } else {
                AdapterError::Unavailable("owned diagnostic".repeat(128))
            }
        });
        drop(retained);
        let result = resolve::<()>(Err(AdmittedError::Adapter(error)), &recorder.policy)
            .await
            .unwrap_err();
        assert_eq!(
            *result,
            if capacity {
                AgentConversationError::CapacityExceeded
            } else {
                AgentConversationError::Unavailable(String::new())
            }
        );
        assert_eq!(root.available_bytes(), idle_bytes(&root));
    }
    for (error, expected) in [
        (
            AdmittedError::Refused(AdmissionRefusal::InvalidPlan { phase: "fixture" }),
            AgentConversationError::InvalidState("invalid recording allocation plan"),
        ),
        (
            AdmittedError::Unsupported("unsupported fixture"),
            AgentConversationError::InvalidState("unsupported fixture"),
        ),
        (
            AdmittedError::Unavailable("unavailable fixture"),
            AgentConversationError::Unavailable(String::new()),
        ),
    ] {
        assert_eq!(
            *resolve::<()>(Err(error), &recorder.policy)
                .await
                .unwrap_err(),
            expected
        );
    }
}

#[tokio::test]
async fn foreign_operation_is_dropped_without_polling_or_recording() {
    let (session_root, session_recorder) = fixture();
    let (other_root, other_recorder) = fixture();
    let service: &dyn AgentUpdateRecorder = &other_recorder;
    let mut session = session_recorder.recording_session();
    let error = service
        .record_output_in_session(
            &AgentId("agt_foreign_recorder".into()),
            7,
            &mut session,
            &ConversationUpdate::Lost { frames: 3 },
            std::time::UNIX_EPOCH,
        )
        .await
        .unwrap_err();
    assert_eq!(
        *error,
        AgentConversationError::InvalidState("invalid recording allocation plan")
    );
    assert_eq!(other_recorder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(other_root.available_bytes(), idle_bytes(&other_root));
    assert_eq!(session_root.available_bytes(), idle_bytes(&session_root));
}

#[tokio::test]
async fn another_attachment_on_the_same_root_cannot_supply_the_recording_operation() {
    let root = IngressBudget::new(512 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        2,
        plan(4096),
        plan(96 * 1024 * 1024),
    )
    .unwrap();
    let process = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let original = process.admit_attachment().unwrap();
    let other = process.admit_attachment().unwrap();
    let recorder = Recorder {
        policy: other.policy().clone(),
        calls: AtomicUsize::new(0),
        fail: false,
        block: false,
    };
    let service: &dyn AgentUpdateRecorder = &recorder;
    let mut session = RecordingSession::new(Some(original.policy().clone()));
    let before = root.available_bytes();
    let error = service
        .restore_recording(&AgentId("agt_foreign_attachment".into()), 7, &mut session)
        .await
        .unwrap_err();
    assert_eq!(
        *error,
        AgentConversationError::InvalidState("invalid recording allocation plan")
    );
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), before);
    drop((error, session, recorder, original, other));
    assert_eq!(root.available_bytes(), idle_bytes(&root));
}

#[tokio::test]
async fn data_pressure_refuses_recording_before_initialization_and_preserves_ack_progress() {
    use layer3_adapter::RuntimeOutputReceipt;
    let root = IngressBudget::new(256 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024)).unwrap(),
    )
    .unwrap();
    let recorder = Recorder {
        policy: policy.clone(),
        calls: AtomicUsize::new(0),
        fail: false,
        block: false,
    };
    let (sender, mut acknowledgements) =
        RuntimeOutputReceipt::admitted_channel_checked(policy.clone(), 1).unwrap();
    let receipt = RuntimeOutputReceipt::new_admitted(
        RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000001".into(),
            cursor: 1,
            native_conversation_id: "synthetic-conversation".into(),
            payload_sha256: "b".repeat(64),
        },
        sender,
    );
    let pressure = policy
        .admit(IngressClass::Data, plan(policy.bounds().data_bytes()))
        .unwrap();
    let available = root.available_bytes();
    let service: &dyn AgentUpdateRecorder = &recorder;
    let mut session = service.recording_session();
    let error = service
        .record_output_in_session(
            &AgentId("agt_data_pressure".into()),
            7,
            &mut session,
            &ConversationUpdate::Lost { frames: 3 },
            std::time::UNIX_EPOCH,
        )
        .await
        .unwrap_err();
    assert_eq!(*error, AgentConversationError::CapacityExceeded);
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), available);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    receipt.acknowledge_admitted().unwrap();
    assert_eq!(*acknowledgements.recv().await.unwrap(), receipt.source);
    assert_eq!(root.available_bytes(), available);
    drop((
        error,
        session,
        receipt,
        acknowledgements,
        recorder,
        pressure,
    ));
    assert_eq!(root.available_bytes(), idle_bytes(&root));
}
