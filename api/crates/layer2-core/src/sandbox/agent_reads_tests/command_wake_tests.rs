use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};

pub(super) struct WakeProbe {
    pub store: Arc<InMemorySandboxStore>,
    pub calls: AtomicUsize,
}

#[async_trait]
impl CommandDiscoveryWake for WakeProbe {
    async fn committed_cancel(&self, agent: &AgentId) {
        let committed = self
            .store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap();
        assert!(
            committed.items.iter().any(|record| {
                record.receipt.envelope.agent_id == *agent
                    && matches!(record.receipt.envelope.command, AgentCommand::Interrupt)
            }),
            "observer must only see committed Cancel work"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

fn cancel(key: &str) -> CommandSubmission {
    CommandSubmission {
        command: AgentCommand::Interrupt,
        ..submission(key)
    }
}

#[tokio::test]
async fn only_new_committed_cancel_notifies_and_policy_builder_preserves_observer() {
    let (store, service, _, _) = targeted_controls::fixture().await;
    let wake = Arc::new(WakeProbe {
        store,
        calls: AtomicUsize::new(0),
    });
    let service = service
        .with_discovery_wake(Some(wake.clone()))
        .with_task_experience(Default::default());
    for request in [submission("message"), cancel("cancel"), cancel("cancel")] {
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request,
            )
            .await
            .unwrap();
    }
    assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
    let mut invalid = cancel("stale");
    invalid.expected_agent_revision = Some(9000);
    assert!(service
        .admit(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            invalid
        )
        .await
        .is_err());
    assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn aborted_admission_before_reservation_does_not_notify() {
    let entered = Arc::new(Notify::new());
    let mut authority = Authority::new();
    authority.gate = Some((entered.clone(), Arc::new(Notify::new())));
    let (store, service, _, _) = setup(authority).await;
    let wake = Arc::new(WakeProbe {
        store: store.clone(),
        calls: AtomicUsize::new(0),
    });
    let service = service.with_discovery_wake(Some(wake.clone()));
    let work = tokio::spawn(async move {
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                cancel("aborted"),
            )
            .await
    });
    entered.notified().await;
    work.abort();
    assert!(work.await.unwrap_err().is_cancelled());
    assert_eq!(wake.calls.load(Ordering::SeqCst), 0);
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

struct ReservationBoundary {
    inner: Arc<InMemorySandboxStore>,
    uncertain: bool,
}

#[async_trait]
impl AgentCommandStore for ReservationBoundary {
    async fn reserve_command(
        &self,
        request: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        let result = self.inner.reserve_command(request).await?;
        if self.uncertain {
            Err(CommandLedgerError::Unavailable)
        } else {
            share(&self.inner, None).await;
            Ok(result)
        }
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_command(id).await
    }
    async fn get_response_command(
        &self,
        id: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        self.inner.get_response_command(id).await
    }
}

#[tokio::test]
async fn confirmed_commit_wakes_before_reply_fence_but_uncertain_commit_uses_polling() {
    for uncertain in [false, true] {
        let (store, _, authority, _) = targeted_controls::fixture().await;
        let wake = Arc::new(WakeProbe {
            store: store.clone(),
            calls: AtomicUsize::new(0),
        });
        let service = AgentCommandService::new(
            Arc::new(ReservationBoundary {
                inner: store.clone(),
                uncertain,
            }),
            Arc::new(ResourceAccessService::new(store.clone(), authority.clone())),
            authority,
        )
        .with_discovery_wake(Some(wake.clone()));
        assert!(service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                cancel("cancel")
            )
            .await
            .is_err());
        assert_eq!(wake.calls.load(Ordering::SeqCst), usize::from(!uncertain));
        assert_eq!(
            store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items
                .len(),
            2
        );
    }
}

#[tokio::test]
async fn targeted_and_public_cancel_notify_once_after_durable_reservation() {
    for public in [false, true] {
        let (store, service, _, request) = targeted_controls::fixture().await;
        let wake = Arc::new(WakeProbe {
            store,
            calls: AtomicUsize::new(0),
        });
        let service = service.with_discovery_wake(Some(wake.clone()));
        for _ in 0..2 {
            if public {
                service
                    .admit_response_control(
                        &context("owner"),
                        "selected-credential",
                        CommandSubmissionSource::Api,
                        request.clone(),
                    )
                    .await
                    .unwrap();
            } else {
                service
                    .admit_targeted_control(
                        &context("owner"),
                        "selected-credential",
                        CommandSubmissionSource::Api,
                        request.clone(),
                    )
                    .await
                    .unwrap();
            }
        }
        assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn atomic_interrupt_with_message_notifies_only_its_new_committed_control() {
    let (store, service, _, control) = targeted_controls::fixture().await;
    let wake = Arc::new(WakeProbe {
        store,
        calls: AtomicUsize::new(0),
    });
    let service = service.with_discovery_wake(Some(wake.clone()));
    let request = InterruptingMessageSubmission {
        agent_id: control.agent_id,
        target: control.target,
        expected_agent_revision: control.expected_agent_revision,
        text: "replacement".into(),
        idempotency_key: "paired-cancel".into(),
    };
    for _ in 0..2 {
        service
            .interrupt_with_message(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request.clone(),
            )
            .await
            .unwrap();
    }
    assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
    let refused = InterruptingMessageSubmission {
        expected_agent_revision: request.expected_agent_revision + 100,
        idempotency_key: "stale-pair".into(),
        ..request
    };
    assert!(service
        .interrupt_with_message(
            &context("owner"),
            "selected-credential",
            CommandSubmissionSource::Api,
            refused
        )
        .await
        .is_err());
    assert_eq!(wake.calls.load(Ordering::SeqCst), 1);
}
