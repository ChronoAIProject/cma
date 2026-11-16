//! Independent media lease fencing at every native route/retry commit point.
use super::live_scope::Scope;
use super::{dispatcher::current_target, Authority, MediaLease, MediaObligationStore};
use crate::{
    delivery::{
        destination::DefaultDestination,
        gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate},
    },
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::{Duration, Instant};
pub(super) struct MediaSendGate {
    store: MediaObligationStore,
    lease: MediaLease,
    authority: Arc<dyn Authority>,
    plan: AdmittedPlan,
    target: StoredTarget,
    version: u64,
    deadline: Instant,
    cancelled: AtomicBool,
    scope: Option<Arc<Scope>>,
    destination: Option<DefaultDestination>,
}
impl MediaSendGate {
    pub fn new(
        store: MediaObligationStore,
        lease: &MediaLease,
        authority: Arc<dyn Authority>,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        version: u64,
        destination: Option<DefaultDestination>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            lease: lease.clone(),
            authority,
            plan: plan.clone(),
            target: target.clone(),
            version,
            deadline: Instant::now() + Duration::from_secs(20),
            cancelled: AtomicBool::new(false),
            scope: None,
            destination,
        })
    }
    pub fn with_scope(mut self: Arc<Self>, scope: Arc<Scope>) -> Arc<Self> {
        Arc::get_mut(&mut self).expect("unshared gate").scope = Some(scope);
        self
    }
    fn live(&self) -> bool {
        let lease = self
            .scope
            .as_ref()
            .map(|s| s.media())
            .unwrap_or_else(|| self.lease.clone());
        let now = (self.store.0.clock)();
        !self.cancelled.load(Ordering::SeqCst)
            && self.scope.as_ref().is_none_or(|s| s.live())
            && self
                .lease
                .partition
                .as_ref()
                .is_none_or(|token| token.live())
            && Instant::now() < self.deadline
            && self
                .lease
                .record
                .ownership
                .as_ref()
                .is_some_and(|o| o.until > now)
            && lease.record.event_expires_at > now
    }
    async fn check(&self) -> Result<(), GateRefusal> {
        if !self.live() {
            return Err(refused());
        }
        self.store
            .0
            .require_reply_open(&self.plan, &self.target)
            .await
            .map_err(|_| refused())?;
        let current = current_target(self.authority.as_ref(), &self.plan, &self.target)
            .await
            .map_err(|_| refused())?;
        if current.version != self.version
            || !self
                .plan
                .current_preparation_source(&current.definition, &self.store.0.keys)
            || self
                .destination
                .as_ref()
                .is_some_and(|d| !d.authorized(&current))
        {
            return Err(refused());
        }
        if let Some(scope) = &self.scope {
            scope.guard().await.map_err(|_| refused())?;
        } else {
            self.store
                .guard_effect(&self.lease)
                .await
                .map_err(|_| refused())?;
        }
        if !self.live() {
            return Err(refused());
        }
        Ok(())
    }
}
impl SendGate for MediaSendGate {
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
        reason: "media_delivery_authority_expired",
        retry_after: None,
    }
}
pub(super) struct CancelMediaSend(pub Arc<MediaSendGate>);
impl Drop for CancelMediaSend {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
}
