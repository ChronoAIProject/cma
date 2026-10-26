use super::{
    dispatcher::current_target,
    tool_reply_store::{ReplyClaim, ReplyStore},
    Authority,
};
use crate::{
    auth::AgentCaller,
    delivery::gate::{AttemptPermit, GateRefusal, RouteTicket, SendGate},
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use futures_util::future::BoxFuture;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::time::Instant;

pub(super) struct ToolContext {
    pub authority: Arc<dyn Authority>,
    pub plan: AdmittedPlan,
    pub target: StoredTarget,
    pub channel_version: u64,
    pub caller: AgentCaller,
}
enum Mode {
    Tool(Box<ToolContext>),
    Automatic(Arc<dyn SendGate>),
}
pub(super) struct ReplyGate {
    store: ReplyStore,
    claim: ReplyClaim,
    mode: Mode,
    expires_at: i64,
    deadline: Instant,
    cancelled: AtomicBool,
}
impl ReplyGate {
    pub fn tool(store: ReplyStore, claim: &ReplyClaim, context: ToolContext) -> Arc<Self> {
        let expires_at = context.target.launch.event.expires_at;
        Self::new(store, claim, Mode::Tool(Box::new(context)), expires_at)
    }
    pub fn automatic(
        store: ReplyStore,
        claim: &ReplyClaim,
        inner: Arc<dyn SendGate>,
        expires_at: i64,
    ) -> Arc<Self> {
        Self::new(store, claim, Mode::Automatic(inner), expires_at)
    }
    fn new(store: ReplyStore, claim: &ReplyClaim, mode: Mode, expires_at: i64) -> Arc<Self> {
        Arc::new(Self {
            store,
            claim: claim.clone(),
            mode,
            expires_at,
            deadline: Instant::now() + super::delivery_gate::SEND_WINDOW,
            cancelled: AtomicBool::new(false),
        })
    }
    fn live(&self) -> bool {
        let now = (self.store.0.clock)();
        !self.cancelled.load(Ordering::SeqCst)
            && self.deadline > Instant::now()
            && self.expires_at > now
            && self.claim.until() > now
    }
    async fn check(&self) -> Result<(), GateRefusal> {
        if !self.live() {
            return Err(refused());
        }
        if let Mode::Tool(context) = &self.mode {
            let current =
                current_target(context.authority.as_ref(), &context.plan, &context.target)
                    .await
                    .map_err(|_| refused())?;
            if current.version != context.channel_version
                || context.caller.consumer(&current).ok().as_ref()
                    != Some(&context.target.launch.consumer)
            {
                return Err(refused());
            }
        }
        self.store.guard(&self.claim).await.map_err(|_| refused())?;
        if !self.live() {
            return Err(refused());
        }
        Ok(())
    }
}
impl SendGate for ReplyGate {
    fn enter_route<'a>(
        &'a self,
        route: &'a str,
    ) -> BoxFuture<'a, Result<RouteTicket, GateRefusal>> {
        Box::pin(async move {
            self.check().await?;
            match &self.mode {
                Mode::Automatic(inner) => inner.enter_route(route).await,
                Mode::Tool(_) => Ok(RouteTicket::empty()),
            }
        })
    }
    fn attempt<'a>(&'a self) -> BoxFuture<'a, Result<AttemptPermit, GateRefusal>> {
        Box::pin(async move {
            self.check().await?;
            match &self.mode {
                Mode::Automatic(inner) => inner.attempt().await,
                Mode::Tool(_) => Ok(AttemptPermit::empty()),
            }
        })
    }
    fn commit(&self) -> bool {
        self.live()
            && match &self.mode {
                Mode::Automatic(inner) => inner.commit(),
                Mode::Tool(_) => true,
            }
    }
    fn progress(&self) {
        if let Mode::Automatic(inner) = &self.mode {
            inner.progress();
        }
    }
}
fn refused() -> GateRefusal {
    GateRefusal {
        reason: "routed_reply_unavailable",
        retry_after: None,
    }
}
pub(super) struct CancelReply(pub Arc<ReplyGate>);
impl Drop for CancelReply {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
}
