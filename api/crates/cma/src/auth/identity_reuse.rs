//! Short success reuse is scoped to ambient identity resolution on selected reads.
use std::{cell::RefCell, future::Future, sync::Arc, time::Duration};

use axum::http::Method;
use layer2_core::StoreError;

use super::{
    identity::VerifiedSubject,
    identity_fallback::{CredentialPurpose, Epoch, TokenHash},
    AuthState,
};

struct Lease {
    key: TokenHash,
    epoch: Epoch,
    age: Duration,
}

struct Scope {
    age: Duration,
    leases: RefCell<Vec<Lease>>,
}

tokio::task_local! { static CURRENT: Scope; }

impl AuthState {
    pub(crate) fn with_identity_reuse(mut self: Arc<Self>, seconds: u64) -> Arc<Self> {
        assert!(
            seconds <= 5,
            "identity reuse must be bounded by five seconds"
        );
        Arc::get_mut(&mut self)
            .expect("auth configured before sharing")
            .identity_reuse_age = Duration::from_secs(seconds);
        self
    }
}

fn permits(method: &Method, path: &str) -> bool {
    if !matches!(*method, Method::GET | Method::HEAD) {
        return false;
    }
    if path == "/auth/session" {
        return method == Method::GET;
    }
    // New and sensitive read surfaces require their own review. In particular,
    // events/streams, secrets, admin and delegated commands do not enter here.
    matches!(
        path,
        "/api/v1/agents"
            | "/api/v1/agents/{id}"
            | "/api/v1/sandboxes"
            | "/api/v1/sandboxes/{id}"
            | "/api/v1/workspaces"
            | "/api/v1/workspaces/{id}"
    )
}

pub(super) async fn scope<T>(
    auth: &AuthState,
    method: &Method,
    path: &str,
    future: impl Future<Output = Result<T, StoreError>>,
) -> Result<T, StoreError> {
    let age = if permits(method, path) {
        auth.identity_reuse_age
    } else {
        Duration::ZERO
    };
    CURRENT
        .scope(
            Scope {
                age,
                leases: RefCell::new(Vec::new()),
            },
            async {
                let result = future.await?;
                // All durable custody/login reads finish first. A logout, refusal or TTL
                // boundary during those reads cannot turn cached evidence into authority.
                let current = CURRENT.with(|scope| {
                    scope.leases.borrow().iter().all(|lease| {
                        auth.identity_store
                            .get_recent(&lease.key, &lease.epoch, lease.age)
                            .is_some()
                    })
                });
                if current {
                    Ok(result)
                } else {
                    Err(super::identity_retry::unavailable())
                }
            },
        )
        .await
}

pub(super) fn used() -> bool {
    CURRENT
        .try_with(|scope| !scope.leases.borrow().is_empty())
        .unwrap_or(false)
}

pub(super) fn lookup(
    auth: &AuthState,
    purpose: CredentialPurpose,
    token: &str,
) -> Option<Arc<VerifiedSubject>> {
    if !matches!(
        purpose,
        CredentialPurpose::BrowserCookie | CredentialPurpose::IndependentBearer
    ) {
        return None;
    }
    CURRENT
        .try_with(|scope| {
            if scope.age.is_zero() {
                return None;
            }
            let key = auth.identity_store.key(purpose, token);
            let epoch = auth.identity_store.epoch();
            let evidence = auth.identity_store.get_recent(&key, &epoch, scope.age);
            if evidence.is_some() {
                scope.leases.borrow_mut().push(Lease {
                    key,
                    epoch,
                    age: scope.age,
                });
                super::identity_observations::cache("hit");
            } else {
                super::identity_observations::cache("miss");
            }
            evidence
        })
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuse_allowlist_excludes_commands_privileged_reads_and_future_routes() {
        for method in [Method::GET, Method::HEAD] {
            for path in [
                "/api/v1/agents",
                "/api/v1/agents/{id}",
                "/api/v1/workspaces/{id}",
                "/api/v1/sandboxes/{id}",
            ] {
                assert!(permits(&method, path));
            }
        }
        for method in [
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ] {
            assert!(!permits(&method, "/api/v1/agents"));
        }
        for path in [
            "/api/v1/agents/",
            "/api/v1/agents/provenance",
            "/api/v1/agents/future-route",
            "unmatched",
            "/api/v1/agents/id/events",
            "/api/v1/agents/id/service-access",
            "/api/v1/admin/settings",
            "/api/v1/organizations",
            "/api/v1/future",
            "/auth/session/",
        ] {
            assert!(!permits(&Method::GET, path));
        }
        assert!(!permits(&Method::HEAD, "/auth/session"));
    }
}
