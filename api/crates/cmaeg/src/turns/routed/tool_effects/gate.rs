use super::{
    model::{Payload, Root, State},
    store::Store,
};
use crate::{
    auth::AgentCaller,
    delivery::gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate},
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::partition::PartitionLease,
    turns::routed::{storage, RoutedToolReplies},
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::Instant;

pub(super) struct Gate {
    pub tools: Arc<RoutedToolReplies>,
    pub plan: AdmittedPlan,
    pub target: StoredTarget,
    pub payload: Payload,
    pub root: Root,
    pub token: PartitionLease,
    pub channel_version: u64,
    pub deadline: Instant,
    pub cancelled: AtomicBool,
}
impl Gate {
    fn live(&self) -> bool {
        !self.cancelled.load(Ordering::SeqCst)
            && self.token.live()
            && Instant::now() < self.deadline
            && self.payload.capability.expires_at > (self.tools.journal.clock)()
    }
    async fn check(&self) -> Result<(), GateRefusal> {
        if !self.live() {
            return Err(refused());
        }
        let current = self
            .tools
            .authorize_delayed(
                AgentCaller {
                    key_hash: self.payload.caller,
                },
                &self.payload.capability,
                &self.plan,
                &self.target,
            )
            .await
            .map_err(|_| refused())?;
        if current.version != self.channel_version {
            return Err(refused());
        }
        self.tools
            .journal
            .guard_partition(&Some(self.token.clone()))
            .await
            .map_err(|_| refused())?;
        let root = Store(self.tools.journal.clone())
            .read(&self.root.id, &self.plan, &self.target)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        if root.version != self.root.version
            || !matches!(root.state, State::Sending)
            || storage::bytes(&root).map_err(|_| refused())?
                != storage::bytes(&self.root).map_err(|_| refused())?
            || !self.live()
        {
            return Err(refused());
        }
        Ok(())
    }
}
impl SendGate for Gate {
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
        reason: "replica_delayed_reply_unavailable",
        retry_after: None,
    }
}
pub(super) struct Cancel(pub Arc<Gate>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
}
