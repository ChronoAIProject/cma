use super::{dispatcher::current_target, Authority, ExecutionError, Journal, Lease};
use crate::{
    delivery::gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate},
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::{Duration, Instant};

pub(super) const SEND_WINDOW: Duration = Duration::from_secs(20);
pub(super) struct DeliveryGate {
    journal: Journal,
    lease: Lease,
    authority: Arc<dyn Authority>,
    plan: AdmittedPlan,
    target: StoredTarget,
    channel_version: u64,
    destination: Option<super::DefaultDestination>,
    origin_webhook: Option<crate::delivery::destination::WebhookDestination>,
    deadline: Instant,
    cancelled: AtomicBool,
}
impl DeliveryGate {
    pub fn new(
        journal: Journal,
        lease: &Lease,
        authority: Arc<dyn Authority>,
        plan: AdmittedPlan,
        target: StoredTarget,
        channel_version: u64,
        destination: Option<super::DefaultDestination>,
    ) -> Arc<Self> {
        Arc::new(Self {
            journal,
            lease: Lease {
                state: lease.state.clone(),
            },
            authority,
            plan,
            target,
            channel_version,
            destination,
            origin_webhook: None,
            deadline: Instant::now() + SEND_WINDOW,
            cancelled: AtomicBool::new(false),
        })
    }
    pub fn with_origin_webhook(
        mut self: Arc<Self>,
        destination: crate::delivery::destination::WebhookDestination,
    ) -> Arc<Self> {
        // Only the constructor's unshared result uses this builder.
        Arc::get_mut(&mut self)
            .expect("unshared delivery gate")
            .origin_webhook = Some(destination);
        self
    }
    fn live(&self) -> bool {
        let now = (self.journal.clock)();
        !self.cancelled.load(Ordering::SeqCst)
            && Instant::now() < self.deadline
            && self.lease.state.ownership.until > now
            && self.lease.state.event_expires_at > now
            && self.lease.state.retained_until > now
    }
    async fn check(&self) -> Result<(), GateRefusal> {
        if !self.live() {
            return Err(refused());
        }
        let current = current_target(self.authority.as_ref(), &self.plan, &self.target)
            .await
            .map_err(|_| refused())?;
        if current.version != self.channel_version
            || self.origin_webhook.as_ref().is_some_and(|destination| {
                !crate::delivery::local_stream::webhook_authorized(&current, destination)
            })
            || self
                .destination
                .as_ref()
                .is_some_and(|destination| !destination.authorized(&current))
        {
            return Err(refused());
        }
        self.journal
            .guard(&self.lease)
            .await
            .map_err(|_: ExecutionError| refused())?;
        if !self.live() {
            return Err(refused());
        }
        Ok(())
    }
}
impl SendGate for DeliveryGate {
    fn enter_route<'a>(
        &'a self,
        _route: &'a str,
    ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
        Box::pin(async move {
            self.check().await?;
            Ok(RouteTicket::empty())
        })
    }
    fn attempt<'a>(&'a self) -> BoxFuture<'a, Result<AttemptPermit, GateRefusal>> {
        Box::pin(async move {
            self.check().await?;
            Ok(AttemptPermit::empty())
        })
    }
    fn commit(&self) -> bool {
        self.live()
    }
    fn progress(&self) {}
}
fn refused() -> GateRefusal {
    GateRefusal {
        reason: "delivery_authority_expired",
        retry_after: None,
    }
}
/// Dropping the owner (including cancellation) disables any still-queued send.
pub(super) struct CancelSend(pub Arc<DeliveryGate>);
impl Drop for CancelSend {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
}
