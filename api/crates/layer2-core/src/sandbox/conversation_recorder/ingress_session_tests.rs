use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, RecordingSession,
};
use crate::sandbox::store::{AgentReopenGuard, SandboxMutationGuard};
use crate::sandbox::{SandboxStatus, SandboxWorkStep};

struct LegacyRecorder {
    calls: AtomicUsize,
    fail: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for LegacyRecorder {
    async fn restore_assembly(
        &self,
        _: &AgentId,
        _: u64,
        assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assembly.content = "retained prefix".into();
        Ok(())
    }

    async fn record_update(
        &self,
        _: &AgentId,
        generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(generation, 7);
        assert!(matches!(update, ConversationUpdate::Lost { frames: 3 }));
        if self.fail {
            return Err(AgentConversationError::Unavailable(
                "retained failure".repeat(128),
            ));
        }
        assembly.content.push_str(" / recorded");
        Ok(AgentUpdateOutcome::quiet())
    }

    async fn record_reopen_update(
        &self,
        id: &AgentId,
        guard: &AgentReopenGuard,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        assert_eq!(guard.agent_fence, 19);
        self.record_update(id, guard.runtime_generation, assembly, update, appended_at)
            .await
    }

    async fn flush_output(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: String,
    ) -> Result<Option<layer3_adapter::RuntimeOutputSource>, AgentConversationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

fn recorder(fail: bool) -> LegacyRecorder {
    LegacyRecorder {
        calls: AtomicUsize::new(0),
        fail,
    }
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(16 * 1024);
    let plan = |n| AllocationPlan::checked("session contract fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(1024), plan(8192)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn guard() -> AgentReopenGuard {
    AgentReopenGuard {
        sandbox: SandboxMutationGuard {
            sandbox_id: crate::SandboxId("sbx_recording_contract".into()),
            owner: crate::SubjectId::new("synthetic-owner"),
            expected_revision: 1,
            status: SandboxStatus::Reviving,
            work_step: SandboxWorkStep::RunLifecycleOperation,
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

#[test]
fn snapshot_operation_requires_an_explicit_store_implementation() {
    use crate::sandbox::ingress_ownership::AdmittedError;
    use crate::sandbox::store::{AgentConversationStore, InMemorySandboxStore};

    let (root, policy) = policy();
    let store = InMemorySandboxStore::default();
    let store: &dyn AgentConversationStore = &store;
    assert!(matches!(
        store
            .agent_message_snapshot_operation(&AgentId("agt_unsupported_snapshot".into()), &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn legacy_session_calls_preserve_mutations_and_the_original_error() {
    let recorder = recorder(false);
    let id = AgentId("agt_session_contract".into());
    let update = ConversationUpdate::Lost { frames: 3 };
    let mut session = recorder.recording_session();
    assert!(session.policy().is_none());
    let restored = recorder
        .restore_session(&id, 7, &mut session)
        .await
        .unwrap();
    assert!(!restored.is_admitted());
    assert_eq!(session.assembly().content, "retained prefix");
    recorder
        .record_session_update(&id, 7, &mut session, &update, String::new())
        .await
        .unwrap();
    let output = recorder
        .record_session_output(&id, 7, &mut session, &update, String::new())
        .await
        .unwrap();
    assert!(!output.is_admitted());
    assert!(output.committed.is_none());
    recorder
        .record_session_reopen(&id, &guard(), &mut session, &update, String::new())
        .await
        .unwrap();
    assert!(recorder
        .flush_session_output(&id, 7, &mut session, String::new())
        .await
        .unwrap()
        .is_none());
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 5);
    assert_eq!(
        session.assembly().content,
        "retained prefix / recorded / recorded / recorded"
    );
    let before = serde_json::to_string(session.assembly()).unwrap();
    let failing = LegacyRecorder {
        calls: AtomicUsize::new(0),
        fail: true,
    };
    let error = failing
        .record_session_update(&id, 7, &mut session, &update, String::new())
        .await
        .unwrap_err();
    assert!(!error.is_admitted());
    assert_eq!(
        *error,
        AgentConversationError::Unavailable("retained failure".repeat(128))
    );
    assert_eq!(serde_json::to_string(session.assembly()).unwrap(), before);
}

#[tokio::test]
async fn legacy_implementations_refuse_enabled_and_already_owned_sessions_before_calls() {
    let recorder = recorder(false);
    let id = AgentId("agt_refused_contract".into());
    let update = ConversationUpdate::Lost { frames: 3 };
    let (root, policy) = policy();
    let mut enabled = RecordingSession::new(Some(policy));
    let mut installed = RecordingSession::new(None);
    installed
        .replace(
            IngressWorkScope::inherit_owned(async {
                AgentMessageAssembly {
                    content: "owned prefix".into(),
                    ..Default::default()
                }
            })
            .await,
        )
        .unwrap();
    for session in [&mut enabled, &mut installed] {
        let before = serde_json::to_string(session.assembly()).unwrap();
        assert!(recorder.restore_session(&id, 7, session).await.is_err());
        assert!(recorder
            .record_session_update(&id, 7, session, &update, String::new())
            .await
            .is_err());
        assert!(recorder
            .record_session_output(&id, 7, session, &update, String::new())
            .await
            .is_err());
        assert!(recorder
            .record_session_reopen(&id, &guard(), session, &update, String::new())
            .await
            .is_err());
        assert!(recorder
            .flush_session_output(&id, 7, session, String::new())
            .await
            .is_err());
        assert_eq!(serde_json::to_string(session.assembly()).unwrap(), before);
    }
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn returned_session_error_keeps_its_enclosing_work_owner_until_drop() {
    let (root, policy) = policy();
    let scope = IngressWorkScope::new(
        policy
            .admit(
                IngressClass::Transformation,
                AllocationPlan::checked("recording error fixture", &[8192]).unwrap(),
            )
            .unwrap(),
    );
    let recorder = recorder(true);
    let mut session = recorder.recording_session();
    let result = scope
        .run(async {
            recorder
                .record_session_update(
                    &AgentId("agt_owned_error".into()),
                    7,
                    &mut session,
                    &ConversationUpdate::Lost { frames: 3 },
                    String::new(),
                )
                .await
                .unwrap_err()
        })
        .await;
    drop(scope);
    assert_eq!(root.available_bytes(), root.capacity_bytes() - 8192);
    assert_eq!(
        **result,
        AgentConversationError::Unavailable("retained failure".repeat(128))
    );
    drop(result);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
