use super::*;
use layer2_core::sandbox::store::AgentStore;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation,
};

#[derive(Clone, Copy)]
enum FailurePoint {
    Hold,
    InvalidHold,
    Conversation,
}

struct DelayedFailure {
    backend: InMemoryHostedBackend,
    point: FailurePoint,
    resume_fails: bool,
}

#[async_trait::async_trait]
impl HostedBackend for DelayedFailure {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.backend.provision(request).await
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let hold = matches!(request, RequestFrame::CheckpointForTeardown(_));
        let conversation = matches!(request, RequestFrame::CaptureAgent(_));
        let resume = matches!(request, RequestFrame::ResumeCheckpointWriters(_));
        if resume && self.resume_fails {
            tokio::time::sleep(Duration::from_secs(90)).await;
            return Err(AdapterError::Unavailable(
                "fixture resume transport failed".into(),
            ));
        }
        let result = self.backend.exchange(handle, request).await;
        if hold {
            tokio::time::sleep(Duration::from_secs(600)).await;
            match self.point {
                FailurePoint::Hold => {
                    return Err(AdapterError::Unavailable(
                        "fixture hold response lost".into(),
                    ))
                }
                FailurePoint::InvalidHold => {
                    let ResponseFrame::CheckpointForTeardown(mut receipt) = result? else {
                        panic!("expected held checkpoint");
                    };
                    receipt.checkpoint.epoch += 1;
                    return Ok(ResponseFrame::CheckpointForTeardown(receipt));
                }
                FailurePoint::Conversation => {}
            }
        }
        if conversation {
            tokio::time::sleep(Duration::from_secs(120)).await;
            return Err(AdapterError::Unavailable(
                "fixture conversation response lost".into(),
            ));
        }
        if resume {
            tokio::time::sleep(Duration::from_secs(90)).await;
        }
        result
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

#[tokio::test(start_paused = true)]
async fn long_capture_failure_retries_after_capture_and_confirmed_writer_resume_finish() {
    failed_capture(FailurePoint::Hold, false).await;
}

#[tokio::test(start_paused = true)]
async fn long_capture_and_failed_writer_resume_retain_authority_with_a_fresh_retry_delay() {
    failed_capture(FailurePoint::Hold, true).await;
}

#[tokio::test(start_paused = true)]
async fn delayed_conversation_capture_failure_includes_all_elapsed_work_before_retry() {
    failed_capture(FailurePoint::Conversation, false).await;
}

#[tokio::test(start_paused = true)]
async fn delayed_invalid_hold_records_terminal_failure_after_writer_resume_finishes() {
    failed_capture(FailurePoint::InvalidHold, false).await;
}

async fn failed_capture(point: FailurePoint, resume_fails: bool) {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let start = start_time(&created);
    let active = harness.drive_active(&created.id, start).await;
    if matches!(point, FailurePoint::Conversation) {
        harness
            .store
            .create_agent(lifecycle_queue_tests::ready_agent(&active, start))
            .await
            .unwrap();
    }
    let now = start + Duration::from_secs(1);
    harness
        .store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            rfc3339_at(now),
            None,
        ))
        .await
        .unwrap();
    let claimed = harness
        .store
        .claim_sandbox_work(
            &active.id,
            "worker-a",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(1860)),
        )
        .await
        .unwrap()
        .unwrap();
    let authority = claimed
        .lifecycle
        .operation
        .as_ref()
        .unwrap()
        .authority()
        .clone();
    let runtime = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let adapter = Arc::new(CodexAdapter::new(
        Arc::new(DelayedFailure {
            backend: harness.backend.clone(),
            point,
            resume_fails,
        }),
        CodexSandboxSpec::default(),
    ));
    let worker = harness.worker_with_adapter("worker-a", adapter);
    let clock = tokio::time::Instant::now();
    worker
        .execute_claimed_for_test(claimed.clone(), now)
        .await
        .unwrap();
    let elapsed = Duration::from_secs(if matches!(point, FailurePoint::Conversation) {
        810
    } else {
        // The adapter also acknowledges writer recovery before returning a hold error.
        780
    });
    assert_eq!(clock.elapsed(), elapsed);
    let finished_at = now + elapsed;
    let stored = harness.stored(&active.id).await;
    assert_eq!(stored.updated_at, rfc3339_at(finished_at));
    assert_eq!(stored.lifecycle.checkpoints, claimed.lifecycle.checkpoints);
    assert_eq!(stored.runtime, claimed.runtime);
    assert_eq!(stored.work.next_fence, claimed.work.next_fence);
    assert!(stored.work.lease.is_none());
    let events = harness
        .store
        .sandbox_events(&active.id, 0, 200)
        .await
        .unwrap();
    let event = events.last().unwrap();
    assert_eq!(event.appended_at, rfc3339_at(finished_at));
    let SandboxEventKind::WorkStepFailed { step, failure } = &event.kind else {
        panic!("expected failure event");
    };
    assert_eq!(*step, SandboxWorkStep::RunLifecycleOperation);
    assert_eq!(
        harness
            .backend
            .snapshot(&runtime)
            .await
            .unwrap()
            .writers_held,
        resume_fails
    );
    if matches!(point, FailurePoint::InvalidHold) {
        assert_eq!(stored.status, SandboxStatus::Failed);
        assert_eq!(stored.work.step, SandboxWorkStep::None);
        return;
    }
    assert_eq!(failure.class, "retryable_transient");
    assert_eq!(stored.status, SandboxStatus::Pausing);
    assert_eq!(stored.work.attempts, claimed.work.attempts + 1);
    let retry_at = finished_at + Duration::from_secs(5);
    assert_eq!(stored.work.ready_at, rfc3339_at(retry_at));
    let next = stored.lifecycle.operation.as_ref().unwrap().authority();
    assert_eq!(next.operation_id, authority.operation_id);
    assert_eq!(next.requested_at, authority.requested_at);
    if resume_fails {
        assert_eq!(next, &authority);
    } else {
        assert_eq!(
            next.writer_resume_generation,
            authority.writer_resume_generation + 1
        );
        assert_ne!(next.writer_resume_token, authority.writer_resume_token);
    }
    assert!(harness
        .store
        .claim_sandbox_work(
            &active.id,
            "worker-b",
            rfc3339_at(retry_at - Duration::from_secs(1)),
            rfc3339_at(retry_at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(harness.stored(&active.id).await, stored);
    let successor = harness
        .store
        .claim_sandbox_work(
            &active.id,
            "worker-b",
            rfc3339_at(retry_at),
            rfc3339_at(retry_at + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        successor.work.lease.as_ref().unwrap().fence > claimed.work.lease.as_ref().unwrap().fence
    );
    assert_eq!(successor.lifecycle, stored.lifecycle);
    assert_eq!(
        worker.execute_claimed_for_test(claimed, retry_at).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(harness.stored(&active.id).await, successor);
}
