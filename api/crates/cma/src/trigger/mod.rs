pub(crate) mod github;

use async_trait::async_trait;
use layer2_core::{
    LaunchAuthority, PublisherAuthority, SubjectId, TriggerAuthorityVerifier, TriggerConnection,
    TriggerConnectionAuthorization, TriggerConnectionManager, TriggerRepositorySelection,
    TriggerRevision, TriggerRevisionContent, TriggerServiceError,
};

/// Explicit adapter used by trusted local mode, where no renewable NyxID
/// subject authority exists. Resource reads still work; provider effects fail
/// with one typed explanation instead of reaching an invented endpoint.
pub(crate) struct UnavailableGithub;

fn unavailable() -> TriggerServiceError {
    TriggerServiceError::Authority(
        "GitHub connections require provider-backed CMA authentication".to_string(),
    )
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
}
