use super::*;
use crate::trigger::{GitHubConnections, GITHUB_SERVICE};

#[derive(Serialize)]
struct RepositoryAccessReport {
    owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    github: Option<GitHubConnections>,
}

pub(super) async fn report(
    State(state): State<Arc<BrowserConnections>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<CheckRequest>,
) -> Result<Response, ConnectionError> {
    let broker = state
        .repository_access
        .as_ref()
        .ok_or(ConnectionError::NotFound)?;
    let services = state.targets.services(&caller, &request.target).await?;
    if !services
        .iter()
        .any(|service| service.as_str() == GITHUB_SERVICE)
    {
        return Err(ConnectionError::Invalid);
    }
    Ok(no_store(RepositoryAccessReport {
        owner: caller.subject.to_string(),
        github: broker.github_connections(&caller.subject).await,
    }))
}
