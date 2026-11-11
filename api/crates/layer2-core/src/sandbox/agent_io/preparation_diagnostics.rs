//! Content-free diagnostics for one bounded command preparation attempt.

use super::AdmittedAgentCommand;
use crate::resource_access::{AccessError, ResourceAccessError, ResourceAccessStoreError};

/// Observe a boundary without formatting errors, credentials, actors or input.
/// Cancellation (including the worker's IO timeout) must also leave evidence.
/// Each invocation emits at most one warning, with fixed kinds and bounded IDs.
pub async fn observe_command_preparation<T, E>(
    command: &AdmittedAgentCommand,
    boundary: &'static str,
    work: impl std::future::Future<Output = Result<T, E>>,
    error_kind: impl FnOnce(&E) -> &'static str,
) -> Result<T, E> {
    let mut observation = Observation {
        command,
        boundary,
        completed: false,
    };
    let result = work.await;
    observation.completed = true;
    if let Err(error) = &result {
        observation.warn(error_kind(error));
    }
    result
}

struct Observation<'a> {
    command: &'a AdmittedAgentCommand,
    boundary: &'static str,
    completed: bool,
}

impl Observation<'_> {
    fn warn(&self, error_kind: &'static str) {
        let envelope = &self.command.receipt.envelope;
        tracing::warn!(
            event = "command_preparation",
            command_id = %envelope.command_id.as_str().chars().take(128).collect::<String>(),
            agent_id = %envelope.agent_id.as_str().chars().take(128).collect::<String>(),
            boundary = self.boundary,
            error_kind,
            "Queued command preparation did not complete successfully"
        );
    }
}

impl Drop for Observation<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.warn("cancelled");
        }
    }
}

pub fn command_access_error_kind(error: &ResourceAccessError) -> &'static str {
    match error {
        ResourceAccessError::Access(AccessError::NotFound)
        | ResourceAccessError::Store(ResourceAccessStoreError::NotFound) => "resource_not_found",
        ResourceAccessError::Access(AccessError::RevalidationRequired) => "revalidation_required",
        ResourceAccessError::Access(_) => "access_refused",
        ResourceAccessError::Store(ResourceAccessStoreError::Conflict) => "authorization_changed",
        ResourceAccessError::Store(ResourceAccessStoreError::Corrupt) => "authorization_corrupt",
        ResourceAccessError::Store(ResourceAccessStoreError::Unavailable) => "store_unavailable",
        ResourceAccessError::Store(_) => "authorization_store_refused",
        ResourceAccessError::Membership(_) => "membership_unavailable",
    }
}
