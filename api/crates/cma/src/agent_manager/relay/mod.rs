//! Authenticated peer relay. Public credentials and durable command IDs survive a hop.
mod authentication;
mod control;
pub(crate) use control::prestop;
mod http;
mod live;
mod maintenance;

use layer2_core::agent_ownership::{
    bounded, Authority, Member, OwnershipCoordinator, OwnershipToken,
};
use layer2_core::sandbox::DurableAgentUpdateRecorder;
use layer2_core::AgentId;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::Semaphore;

pub(super) const PREFIX: &str = "/internal/agent-manager/v1";
pub(super) const PROOF: &str = "x-cma-agent-manager-relay";
pub(super) const MAX_STREAMS: usize = 256;
pub(super) const MAX_FRAME: usize = 128 * 1024;

pub(crate) struct Relay {
    coordinator: Arc<OwnershipCoordinator>,
    maintenance: std::sync::OnceLock<maintenance::Maintenance>,
    updates: Weak<DurableAgentUpdateRecorder>,
    authentication: authentication::Authentication,
    client: reqwest::Client,
    slots: Arc<Semaphore>,
}

pub(super) struct Destination {
    member: Member,
    token: OwnershipToken,
    authority: Arc<Authority>,
}

impl Relay {
    pub(crate) fn new(
        coordinator: Arc<OwnershipCoordinator>,
        updates: &Arc<DurableAgentUpdateRecorder>,
        key: ring::hmac::Key,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            coordinator,
            maintenance: std::sync::OnceLock::new(),
            updates: Arc::downgrade(updates),
            authentication: authentication::Authentication::new(key),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .read_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| "peer relay client is unavailable")?,
            slots: Arc::new(Semaphore::new(MAX_STREAMS)),
        }))
    }

    async fn destination(&self, id: &AgentId) -> Result<Destination, ()> {
        if !self.coordinator.is_ready() {
            return Err(());
        }
        let started = tokio::time::Instant::now();
        let membership = bounded(self.coordinator.store.membership())
            .await
            .map_err(|_| ())?;
        membership.require_compatible().map_err(|_| ())?;
        let owner = bounded(self.coordinator.store.owner(id))
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        let member = membership
            .members
            .into_iter()
            .find(|member| {
                member.instance_id == owner.token.instance_id
                    && !member.draining
                    && member.is_live(membership.now_ms)
            })
            .ok_or(())?;
        let remaining = owner.expires_at_ms.min(member.expires_at_ms) - membership.now_ms;
        if remaining <= 0 {
            return Err(());
        }
        let authority = if member.instance_id == self.coordinator.instance_id {
            self.coordinator
                .acquire(id)
                .await
                .map_err(|_| ())?
                .ok_or(())?
        } else {
            Authority::new(
                owner.token.clone(),
                started + Duration::from_millis(remaining as u64),
            )
        };
        if !authority.is_current() || authority.token != owner.token {
            return Err(());
        }
        Ok(Destination {
            member,
            token: owner.token,
            authority,
        })
    }

    pub(crate) fn router(
        self: &Arc<Self>,
        router: axum::Router,
        manager: Arc<crate::agent_manager::AgentManager>,
    ) -> axum::Router {
        let router = self.routes(router);
        control::router(router, manager, self.clone())
    }

    fn routes(self: &Arc<Self>, router: axum::Router) -> axum::Router {
        router
            .merge(
                axum::Router::new()
                    .route(
                        "/internal/agent-manager/v1/live/{agent}",
                        axum::routing::get(live::serve),
                    )
                    .route(
                        "/internal/agent-manager/v1/control/{agent}",
                        axum::routing::post(maintenance::serve),
                    )
                    .with_state(self.clone()),
            )
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                http::route,
            ))
    }
}

#[cfg(test)]
mod tests;
