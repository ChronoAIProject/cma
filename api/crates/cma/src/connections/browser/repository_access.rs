use super::*;
use crate::trigger::{GitHubConnections, GitHubRepositoryNeed, GitHubScopeCheck, GITHUB_SERVICE};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RepositoryAccessRequest {
    target: ConnectionTarget,
    #[serde(default, rename = "active")]
    _active: Option<ActiveConnection>,
    #[serde(default)]
    github_operation: Option<GitHubRepositoryNeed>,
}

#[derive(Serialize)]
struct GitHubScopeReport {
    operation: GitHubRepositoryNeed,
    #[serde(flatten)]
    check: GitHubScopeCheck,
}

#[derive(Serialize)]
struct RepositoryAccessReport {
    owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    github: Option<GitHubConnections>,
    #[serde(skip_serializing_if = "Option::is_none")]
    github_scope_check: Option<GitHubScopeReport>,
}

pub(super) async fn report(
    State(state): State<Arc<BrowserConnections>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(request): Json<RepositoryAccessRequest>,
) -> Result<Response, ConnectionError> {
    let broker = state
        .repository_access
        .as_ref()
        .ok_or(ConnectionError::NotFound)?;
    if request.github_operation.is_some() && !state.repository_scope_check {
        return Err(ConnectionError::NotFound);
    }
    let services = state.targets.services(&caller, &request.target).await?;
    if !services
        .iter()
        .any(|service| service.as_str() == GITHUB_SERVICE)
    {
        return Err(ConnectionError::Invalid);
    }
    let github = broker.github_connections(&caller.subject).await;
    let github_scope_check = request.github_operation.map(|operation| GitHubScopeReport {
        operation,
        check: github
            .as_ref()
            .map_or(GitHubScopeCheck::VerificationUnavailable, |github| {
                github.scope_check(operation)
            }),
    });
    Ok(no_store(RepositoryAccessReport {
        owner: caller.subject.to_string(),
        github,
        github_scope_check,
    }))
}
