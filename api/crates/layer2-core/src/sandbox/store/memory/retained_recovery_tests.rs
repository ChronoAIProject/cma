use super::*;
use crate::sandbox::lifecycle_test_support::{agent, log, NOW};
use crate::sandbox::runtime_update_test_support::native;
use crate::sandbox::{
    AgentStatus, RetainedRecoveryDisposition as D, RetainedRecoveryError,
    RetainedRecoveryInspector, RetainedRecoveryService, RetainedRecoverySnapshot, SandboxWorkStep,
    TypedFailure,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn fixture() -> (Arc<InMemorySandboxStore>, crate::Caller, SandboxId) {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut sandbox = native();
    sandbox.runtime.as_mut().unwrap().runtime_version =
        format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    sandbox.status = SandboxStatus::Failed;
    sandbox.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "private provider detail".into(),
    });
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    let id = sandbox.id.clone();
    let caller = crate::Caller {
        subject: sandbox.owner.clone(),
        authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
    };
    let mut child = agent("agt_retained", "original-conversation", 1);
    child.status = AgentStatus::Paused;
    child.conversation.runtime_generation = Some(1);
    {
        let mut state = store.lock();
        state.sandboxes.insert(id.clone(), sandbox);
        state
            .agent_logs
            .insert(child.id.clone(), vec![log(child.id.as_str(), 1, NOW)]);
        state.agents.insert(child.id.clone(), child);
    }
    (store, caller, id)
}

struct Inspector {
    store: Arc<InMemorySandboxStore>,
    calls: AtomicUsize,
    change: bool,
    change_owner: bool,
    wait: bool,
}

#[async_trait]
impl RetainedRecoveryInspector for Inspector {
    async fn inspect(&self, snapshot: &RetainedRecoverySnapshot) -> D {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.change {
            self.store
                .lock()
                .sandboxes
                .get_mut(&snapshot.sandbox.id)
                .unwrap()
                .revision += 1;
        }
        if self.change_owner {
            self.store
                .lock()
                .sandboxes
                .get_mut(&snapshot.sandbox.id)
                .unwrap()
                .owner = crate::SubjectId("new-owner".into());
        }
        if self.wait {
            std::future::pending::<()>().await;
        }
        D::ExactBootReplacement
    }
}

fn inspector(store: &Arc<InMemorySandboxStore>, change: bool, wait: bool) -> Arc<Inspector> {
    Arc::new(Inspector {
        store: store.clone(),
        calls: AtomicUsize::new(0),
        change,
        change_owner: false,
        wait,
    })
}

#[tokio::test]
async fn owner_change_during_native_observation_revokes_the_result() {
    let (store, caller, id) = fixture();
    let evidence = Arc::new(Inspector {
        store: store.clone(),
        calls: AtomicUsize::new(0),
        change: false,
        change_owner: true,
        wait: false,
    });
    let service = RetainedRecoveryService::new(store, evidence);
    assert!(matches!(
        service.assess(&caller, &id).await,
        Err(RetainedRecoveryError::NotFound)
    ));
}

#[tokio::test]
async fn owner_read_is_default_closed_and_cannot_mutate_records_or_logs() {
    let (store, caller, id) = fixture();
    let evidence = inspector(&store, false, false);
    let service = RetainedRecoveryService::new(store.clone(), evidence.clone());
    let before = store.retained_runtime_recovery_snapshot(&id).await.unwrap();
    let logs = store.lock().agent_logs.clone();
    let events = store.lock().sandbox_events.clone();
    let result = service.assess(&caller, &id).await.unwrap();
    assert_eq!(result.disposition, D::ExactBootReplacement);
    assert!(!result.admission_enabled);
    assert_eq!(evidence.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.retained_runtime_recovery_snapshot(&id).await.unwrap(),
        before
    );
    assert_eq!(store.lock().agent_logs, logs);
    assert_eq!(store.lock().sandbox_events, events);
    let public = serde_json::to_string(&result).unwrap();
    assert!(!public.contains(id.as_str()));
    assert!(!public.contains("private"));
}

#[tokio::test]
async fn authorization_precedes_child_history_and_native_inspection() {
    let (store, mut caller, id) = fixture();
    let evidence = inspector(&store, false, false);
    let service = RetainedRecoveryService::new(store.clone(), evidence.clone());
    // A corrupt log would otherwise fail inspection, but is never disclosed to
    // another owner or a caller lacking the capability.
    store.lock().agent_logs.values_mut().next().unwrap()[0].seq = 99;
    caller.subject = crate::SubjectId("another-owner".into());
    assert!(matches!(
        service.assess(&caller, &id).await,
        Err(RetainedRecoveryError::NotFound)
    ));
    caller.authorization = crate::Authorization::new([]);
    assert!(matches!(
        service.assess(&caller, &id).await,
        Err(RetainedRecoveryError::Forbidden)
    ));
    assert_eq!(evidence.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_snapshot_after_provider_read_is_a_fixed_refusal() {
    let (store, caller, id) = fixture();
    let service = RetainedRecoveryService::new(store.clone(), inspector(&store, true, false));
    let result = service.assess(&caller, &id).await.unwrap();
    assert_eq!(result.disposition, D::AuthorityChanged);
    assert!(!result.admission_enabled);
}

#[tokio::test(start_paused = true)]
async fn cancellation_and_deadline_release_one_nonqueued_permit() {
    let (store, caller, id) = fixture();
    let evidence = inspector(&store, false, true);
    let service = Arc::new(RetainedRecoveryService::new(store, evidence.clone()));
    let launch = || {
        let (service, caller, id) = (service.clone(), caller.clone(), id.clone());
        tokio::spawn(async move { service.assess(&caller, &id).await })
    };
    let first = launch();
    tokio::task::yield_now().await;
    assert!(matches!(
        service.assess(&caller, &id).await,
        Err(RetainedRecoveryError::Unavailable)
    ));
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let second = launch();
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(20)).await;
    let result = second.await.unwrap().unwrap();
    assert_eq!(result.disposition, D::EvidenceUnavailable);
    assert_eq!(evidence.calls.load(Ordering::SeqCst), 2);
    let third = launch();
    tokio::task::yield_now().await;
    assert_eq!(evidence.calls.load(Ordering::SeqCst), 3);
    third.abort();
    assert!(third.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn diagnostic_read_accepts_published_history_without_expanding_original_admission() {
    let (store, _, id) = fixture();
    store.lock().agent_logs.values_mut().next().unwrap()[0].kind =
        AgentLogKind::ConversationOpened {
            conversation_id: "original-conversation".into(),
            runtime_generation: 1,
        };
    assert!(store.retained_runtime_recovery_snapshot(&id).await.is_ok());
    assert!(store
        .unpublished_runtime_recovery_snapshot(&id)
        .await
        .is_err());
    store.lock().agent_logs.values_mut().next().unwrap()[0].seq = 2;
    assert!(store.retained_runtime_recovery_snapshot(&id).await.is_err());
}
