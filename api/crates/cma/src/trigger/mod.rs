pub(crate) mod capability_signing;
pub(crate) mod capability_worker;
pub(crate) mod github;
pub(crate) mod github_effects;
pub(crate) mod observability;
mod pull_request_status;
pub(crate) mod scheduler;
pub(crate) mod worker;
pub(crate) mod workspace_diff;

use async_trait::async_trait;
use layer2_core::{
    Launch, LaunchAuthority, PublisherAuthority, SubjectId, TriggerAuthorityVerifier,
    TriggerConnection, TriggerConnectionAuthorization, TriggerConnectionManager,
    TriggerRepositorySelection, TriggerRevision, TriggerRevisionContent, TriggerServiceError,
};

/// Durable routing identity for the first production Trigger runtime lane.
///
/// Never rename this in place after a Launch has recorded it. A future lane is
/// a new adapter ID kept alongside this one until session and cleanup-debt
/// inventory proves this generation retired.
pub(crate) const TRIGGER_KERNEL_ID: &str = "codex-trigger-native-v1";

/// Pre-versioned Trigger routing identity retained only for owner reads,
/// normal README re-click, cancellation, reconciliation, revocation, and
/// cleanup. New review, Start, provisioning, and provider polling compare
/// against `TRIGGER_KERNEL_ID`, so this identity cannot authorize fresh work.
pub(crate) const LEGACY_TRIGGER_KERNEL_ID: &str = "codex";

/// Whether a retained Launch carries the complete admission authority for new
/// work on the current Trigger runtime lane. Historical or partial snapshots
/// remain available only to reconciliation, revocation, and fenced cleanup.
pub(crate) fn has_current_trigger_runtime_admission(launch: &Launch) -> bool {
    matches!(
        launch.runtime_admission_route(
            TRIGGER_KERNEL_ID,
            LEGACY_TRIGGER_KERNEL_ID,
            &layer2_core::CMA_TRIGGER_REQUIRED_CAPABILITIES,
        ),
        layer2_core::RuntimeAdmissionRoute::Current
    )
}

/// Explicit adapter used by trusted local mode, where no renewable NyxID
/// subject authority exists. Resource reads still work; provider effects fail
/// with one typed explanation instead of reaching an invented endpoint.
pub(crate) struct UnavailableGithub;

fn unavailable() -> TriggerServiceError {
    layer2_core::TriggerAuthorityError::ProviderAuthenticationUnavailable.into()
}

#[async_trait]
impl TriggerAuthorityVerifier for UnavailableGithub {
    async fn verify_publisher(
        &self,
        _subject: &SubjectId,
        _content: &TriggerRevisionContent,
        _selected_service_id: &str,
    ) -> Result<PublisherAuthority, TriggerServiceError> {
        Err(unavailable())
    }

    async fn verify_launch(
        &self,
        _subject: &SubjectId,
        _revision: &TriggerRevision,
        _selected_service_id: &str,
        _previous: Option<&layer2_core::ConnectionSnapshot>,
    ) -> Result<LaunchAuthority, TriggerServiceError> {
        Err(unavailable())
    }
}

#[async_trait]
impl TriggerConnectionManager for UnavailableGithub {
    async fn list_connections(
        &self,
        _subject: &SubjectId,
    ) -> Result<Vec<TriggerConnection>, TriggerServiceError> {
        Err(unavailable())
    }

    async fn begin_connection(
        &self,
        _subject: &SubjectId,
        _intent_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Err(unavailable())
    }

    async fn reconnect(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnectionAuthorization, TriggerServiceError> {
        Err(unavailable())
    }

    async fn preflight_connection(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
    ) -> Result<TriggerConnection, TriggerServiceError> {
        Err(unavailable())
    }

    async fn resolve_repository(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        _repository_id: u64,
        _selected_ref: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        Err(unavailable())
    }

    async fn resolve_repository_locator(
        &self,
        _subject: &SubjectId,
        _service_id: &str,
        _owner: &str,
        _name: &str,
    ) -> Result<TriggerRepositorySelection, TriggerServiceError> {
        Err(unavailable())
    }
}
