//! Optional owner-key lane. Never inferred from an empty current-record scan.
use super::CreatorAuthError;
use crate::{
    records::RecordError,
    worker_credentials::{canonical_digest, WorkerHistory},
};
use axum::http::HeaderMap;
use cma_service_auth::{ServiceKeyAuth, ServiceKeyPrincipal};
use futures_util::future::BoxFuture;
use std::{fmt, sync::Arc};

pub trait WorkerClassifier: Send + Sync {
    fn was_worker<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<bool, RecordError>>;
}

#[derive(Clone)]
pub struct OwnerKeyAuthenticator {
    verifier: ServiceKeyAuth,
    history: Arc<dyn WorkerHistory>,
    manager: Arc<dyn WorkerClassifier>,
    cutover_id: String,
}

impl fmt::Debug for OwnerKeyAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerKeyAuthenticator")
            .field("cutover_id", &self.cutover_id)
            .finish_non_exhaustive()
    }
}

impl OwnerKeyAuthenticator {
    pub fn new(
        verifier: ServiceKeyAuth,
        history: Arc<dyn WorkerHistory>,
        manager: Arc<dyn WorkerClassifier>,
        cutover_id: String,
    ) -> Self {
        Self {
            verifier,
            history,
            manager,
            cutover_id,
        }
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        hash: &str,
        permission: &str,
    ) -> Result<ServiceKeyPrincipal, CreatorAuthError> {
        let principal = self
            .verifier
            .authenticate(headers)
            .await
            .map_err(super::creator::service_error)?;
        principal
            .require(permission)
            .map_err(super::creator::service_error)?;
        let hash = canonical_digest(hash).map_err(|_| CreatorAuthError::NotAuthenticated)?;
        // Re-read both custody authorities after introspection, on every request.
        // Unknown or failed transport is never equivalent to an owner credential.
        if self
            .manager
            .was_worker(hash)
            .await
            .map_err(|_| CreatorAuthError::ProviderUnavailable)?
            || self
                .history
                .was_worker(hash)
                .await
                .map_err(|_| CreatorAuthError::ProviderUnavailable)?
        {
            return Err(CreatorAuthError::NotAuthenticated);
        }
        let cutover = self
            .history
            .cutover()
            .await
            .map_err(|_| CreatorAuthError::ProviderUnavailable)?
            .ok_or(CreatorAuthError::NotAuthenticated)?;
        if !cutover.admits(&self.cutover_id, principal.issued_at) {
            return Err(CreatorAuthError::NotAuthenticated);
        }
        Ok(principal)
    }
}

#[cfg(test)]
pub(crate) mod tests;
