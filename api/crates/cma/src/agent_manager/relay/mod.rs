//! Authenticated peer relay. Public credentials and durable command IDs survive a hop.
mod authentication;
mod control;
pub(crate) use control::{prestop, DRAIN};
mod file_identity;
mod file_read;
mod file_write;
mod forward;
mod handoff_recovery;
mod http;
mod ingress;
mod live;
mod relay_diagnostics;
#[cfg(test)]
pub(crate) use live::admitted_observation;
mod maintenance;
mod observation;
pub(crate) mod public_access;
pub(crate) use public_access::PublicAccess;
mod queued_cancellation;
pub(crate) use queued_cancellation::{
    Charged as ChargedQueuedCancellation, DeferQueuedCancellationActivity,
};
mod reopen;
pub(crate) use reopen::{output_refused, ReopenCall, OUTPUT_REFUSED};

use layer2_core::agent_ownership::{
    bounded, Authority, Member, OwnershipCoordinator, OwnershipToken,
};
use layer2_core::sandbox::DurableAgentUpdateRecorder;
use layer2_core::AgentId;
use relay_diagnostics::Refusal;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::Semaphore;

mod call_failure;
mod capacity;

pub(super) const PREFIX: &str = "/internal/agent-manager/v1";
pub(super) const PROOF: &str = "x-cma-agent-manager-relay";
/// The live admission bound is the same deadline used to establish a remote
/// relay connection; an unanswered owner falls back to durable replay.
pub(super) const LIVE_ADMISSION_DEADLINE: Duration = Duration::from_secs(3);
#[cfg(test)]
pub(super) const MAX_STREAMS: usize = 256;
pub(super) const MAX_FRAME: usize = 128 * 1024;
pub(super) const MAX_BUFFERED_BODY_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct Relay {
    coordinator: Arc<OwnershipCoordinator>,
    observation: std::sync::OnceLock<observation::Observation>,
    observation_open_trace: std::sync::OnceLock<()>,
    maintenance: std::sync::OnceLock<maintenance::Maintenance>,
    reopen: std::sync::OnceLock<Arc<crate::sandbox::worker::SandboxWorker>>,
    updates: Weak<DurableAgentUpdateRecorder>,
    authentication: authentication::Authentication,
    public_access: std::sync::OnceLock<Arc<dyn public_access::PublicAuthorization>>,
    settings_owner_routing: std::sync::OnceLock<()>,
    file_preview_peer_identity: std::sync::OnceLock<()>,
    client: reqwest::Client,
    reopen_client: reqwest::Client,
    slots: Arc<Semaphore>,
    capacity: crate::config::RelayCapacityConfig,
    capacity_gate: std::sync::Mutex<()>,
    body_budget: Arc<Semaphore>,
}

#[derive(Clone)]
pub(super) struct Destination {
    member: Member,
    token: OwnershipToken,
    authority: Arc<Authority>,
}

impl Relay {
    #[cfg(test)]
    pub(crate) fn new(
        coordinator: Arc<OwnershipCoordinator>,
        updates: &Arc<DurableAgentUpdateRecorder>,
        key: ring::hmac::Key,
    ) -> Result<Arc<Self>, String> {
        Self::with_capacity(coordinator, updates, key, Default::default())
    }
    pub(crate) fn with_capacity(
        coordinator: Arc<OwnershipCoordinator>,
        updates: &Arc<DurableAgentUpdateRecorder>,
        key: ring::hmac::Key,
        capacity: crate::config::RelayCapacityConfig,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            coordinator,
            observation: std::sync::OnceLock::new(),
            observation_open_trace: std::sync::OnceLock::new(),
            maintenance: std::sync::OnceLock::new(),
            reopen: std::sync::OnceLock::new(),
            updates: Arc::downgrade(updates),
            authentication: authentication::Authentication::new(key),
            public_access: std::sync::OnceLock::new(),
            settings_owner_routing: std::sync::OnceLock::new(),
            file_preview_peer_identity: std::sync::OnceLock::new(),
            client: peer_client()
                .read_timeout(Duration::from_secs(10))
                .build()
                .map_err(|_| "peer relay client is unavailable")?,
            // The owner sends its reply only after the retained conversation
            // opens. The reopen call supplies the absolute response deadline.
            reopen_client: peer_client()
                .build()
                .map_err(|_| "peer reopen client is unavailable")?,
            slots: Arc::new(Semaphore::new(capacity.limit)),
            capacity,
            capacity_gate: std::sync::Mutex::new(()),
            body_budget: Arc::new(Semaphore::new(MAX_BUFFERED_BODY_BYTES)),
        }))
    }

    pub(crate) fn install_public_access(&self, access: Arc<PublicAccess>) -> Result<(), String> {
        self.public_access
            .set(access)
            .map_err(|_| "public relay authorization already installed".into())
    }

    pub(crate) fn enable_settings_owner_routing(&self) {
        self.settings_owner_routing.get_or_init(|| ());
    }

    pub(crate) fn enable_file_write_handoff(&self) {
        self.authentication.enable_file_write_handoff();
    }

    pub(crate) fn enable_file_preview_peer_identity(&self) {
        self.file_preview_peer_identity.get_or_init(|| ());
    }

    async fn destination(&self, id: &AgentId) -> Result<Destination, Refusal> {
        if !self.coordinator.is_ready() {
            return Err(Refusal::CoordinatorNotReady);
        }
        let started = tokio::time::Instant::now();
        let membership = bounded(self.coordinator.store.membership())
            .await
            .map_err(|_| Refusal::MembershipUnavailable)?;
        membership
            .require_compatible()
            .map_err(|_| Refusal::MembershipUnavailable)?;
        let owner = bounded(self.coordinator.store.owner(id))
            .await
            .map_err(|_| Refusal::OwnerLookupUnavailable)?
            .ok_or(Refusal::OwnerMissing)?;
        let member = membership
            .members
            .into_iter()
            .find(|member| {
                member.instance_id == owner.token.instance_id
                    && !member.draining
                    && member.is_live(membership.now_ms)
            })
            .ok_or(Refusal::OwnerMemberUnavailable)?;
        let remaining = owner.expires_at_ms.min(member.expires_at_ms) - membership.now_ms;
        if remaining <= 0 {
            return Err(Refusal::AuthorityChanged);
        }
        let authority = if member.instance_id == self.coordinator.instance_id {
            self.coordinator
                .acquire(id)
                .await
                .map_err(|_| Refusal::AuthorityChanged)?
                .ok_or(Refusal::AuthorityChanged)?
        } else {
            Authority::new(
                owner.token.clone(),
                started + Duration::from_millis(remaining as u64),
            )
        };
        if !authority.is_current() || authority.token != owner.token {
            return Err(Refusal::AuthorityChanged);
        }
        Ok(Destination {
            member,
            token: owner.token,
            authority,
        })
    }

    #[cfg(test)]
    pub(crate) fn authenticated_routes(
        self: &Arc<Self>,
        router: axum::Router,
        auth: Arc<crate::auth::AuthState>,
    ) -> axum::Router {
        self.authenticated_control_routes(router, auth, None)
    }

    pub(crate) fn authenticated_control_routes(
        self: &Arc<Self>,
        router: axum::Router,
        auth: Arc<crate::auth::AuthState>,
        admission: Option<Arc<dyn crate::control_admission::LifecycleAdmissionPolicy>>,
    ) -> axum::Router {
        let router = router.layer(axum::middleware::from_fn_with_state(
            self.clone(),
            http::route,
        ));
        let router = match admission {
            Some(policy) => router.layer(axum::middleware::from_fn_with_state(
                policy,
                crate::control_admission::enforce,
            )),
            None => router,
        };
        let router = router
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ))
            .layer(axum::middleware::from_fn(ingress::capture));
        if self.file_preview_peer_identity.get().is_some() {
            router.layer(axum::middleware::from_fn_with_state(
                self.clone(),
                file_identity::receive,
            ))
        } else {
            router
        }
    }

    pub(crate) fn file_preview_identity_route(router: axum::Router) -> axum::Router {
        router.layer(axum::middleware::from_fn(ingress::file_preview_identity))
    }

    pub(crate) fn internal_router(
        self: &Arc<Self>,
        router: axum::Router,
        manager: Arc<crate::agent_manager::AgentManager>,
    ) -> axum::Router {
        let router = router.merge(self.routes(axum::Router::new()));
        control::router(router, manager, self.clone())
    }

    #[cfg(test)]
    fn router(
        self: &Arc<Self>,
        router: axum::Router,
        manager: Arc<crate::agent_manager::AgentManager>,
    ) -> axum::Router {
        control::router(self.routes(router), manager, self.clone())
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
                    .route(
                        "/internal/agent-manager/v1/control/{agent}/reopen",
                        axum::routing::post(reopen::serve),
                    )
                    .with_state(self.clone()),
            )
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                http::route,
            ))
    }
}

fn peer_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
}

#[cfg(test)]
pub(crate) mod tests;
