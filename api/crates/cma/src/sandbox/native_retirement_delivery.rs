use std::{sync::Arc, time::Duration};

use layer2_core::sandbox::{store::SandboxStoreError, Agent, Sandbox};
use layer3_adapter::receiver_failure::ReceiverFailureCapture;
use tokio::sync::oneshot;

use crate::{
    agent_lifecycle::LifecycleDrain,
    store::{native_retirement_action::capture::BoundCapture, MongoSandboxStore},
};

#[derive(Clone)]
pub(crate) struct CaptureDelivery {
    pub(super) store: Arc<MongoSandboxStore>,
    pub(crate) tasks: LifecycleDrain,
}

impl CaptureDelivery {
    pub(crate) fn new(store: Arc<MongoSandboxStore>, capacity: usize) -> Self {
        Self {
            store,
            tasks: LifecycleDrain::completing(capacity),
        }
    }

    pub(super) async fn bind(
        &self,
        parent: &Sandbox,
        child: &Agent,
    ) -> Result<(BoundCapture, PendingCapture), SandboxStoreError> {
        let (send, receive) = oneshot::channel::<(BoundCapture, ReceiverFailureCapture)>();
        let store = self.store.clone();
        if !self.tasks.spawn(async move {
            if let Ok((bound, capture)) = receive.await {
                if let Some(evidence) = capture.sealed() {
                    let reported = tokio::time::timeout(
                        Duration::from_secs(4),
                        store.complete_retirement_capture(&bound, evidence),
                    )
                    .await;
                    if !matches!(reported, Ok(Ok(()))) {
                        tracing::debug!("retirement receiver capture delivery was not confirmed");
                    }
                }
            }
        }) {
            return Err(SandboxStoreError::Conflict);
        }
        let offer = self.store.retirement_capture_offer(parent, child).await?;
        let bound = self
            .store
            .bind_retirement_capture(child, offer, uuid::Uuid::new_v4())
            .await?;
        Ok((
            bound,
            PendingCapture {
                send: Some(send),
                report: None,
            },
        ))
    }
}

pub(super) struct PendingCapture {
    send: Option<oneshot::Sender<(BoundCapture, ReceiverFailureCapture)>>,
    report: Option<(BoundCapture, ReceiverFailureCapture)>,
}

impl PendingCapture {
    pub(super) fn scope<F: std::future::Future>(
        &mut self,
        bound: BoundCapture,
        work: F,
    ) -> impl std::future::Future<Output = F::Output> + use<F> {
        let (capture, scoped) = bound.capture(work);
        self.report = Some((bound, capture));
        scoped
    }

    pub(super) fn sealed(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|(_, capture)| capture.sealed().is_some())
    }
}

impl Drop for PendingCapture {
    fn drop(&mut self) {
        if let (Some(send), Some(report)) = (self.send.take(), self.report.take()) {
            // Sending is synchronous, including when authority drops the waiter.
            let _ = send.send(report);
        }
    }
}
