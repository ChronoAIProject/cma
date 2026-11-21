//! Best-effort revocation of an ended session's refresh token. At most four
//! revocations run at once; beyond that one is dropped and counted, and the
//! token then simply expires at NyxID. Ending a session never waits for it.
use std::{future::Future, sync::Arc};

use cmaeg_core::channel::Secret;
use tokio::sync::Semaphore;

use super::{oidc::EndpointCell, session::SessionCell, token::TokenClient};
use crate::telemetry::names;

const CONCURRENT_REVOCATIONS: usize = 4;

#[derive(Clone)]
pub(crate) struct Revoker {
    tokens: TokenClient,
    endpoints: Arc<EndpointCell>,
    permits: Arc<Semaphore>,
}

impl Revoker {
    pub(crate) fn new(tokens: TokenClient, endpoints: Arc<EndpointCell>) -> Self {
        Self {
            tokens,
            endpoints,
            permits: Arc::new(Semaphore::new(CONCURRENT_REVOCATIONS)),
        }
    }

    /// Revoke `cell`'s refresh token in the background. The task reads the
    /// token under the session's `auth` lock, so a refresh in flight finishes
    /// first and its rotated token is the one revoked.
    pub(crate) fn revoke(&self, cell: Arc<SessionCell>) {
        self.spawn(async move { cell.lock_auth().await.refresh().clone() });
    }

    /// Revoke a refresh token held outside any session.
    pub(crate) fn revoke_token(&self, refresh: Secret) {
        self.spawn(async move { refresh });
    }

    fn spawn(&self, refresh: impl Future<Output = Secret> + Send + 'static) {
        let (Ok(permit), Ok(runtime)) = (
            self.permits.clone().try_acquire_owned(),
            tokio::runtime::Handle::try_current(),
        ) else {
            count("dropped");
            return;
        };
        let Some(endpoints) = self.endpoints.get() else {
            count("dropped");
            return;
        };
        let tokens = self.tokens.clone();
        runtime.spawn(async move {
            let _permit = permit;
            let refresh = refresh.await;
            let revoked = tokens.revoke(&endpoints.revocation, &refresh).await;
            count(if revoked { "ok" } else { "failed" });
        });
    }
}

fn count(outcome: &'static str) {
    metrics::counter!(names::CONSOLE_REVOCATIONS, names::OUTCOME => outcome).increment(1);
}
