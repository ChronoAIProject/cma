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
    #[serde(default)]
    github_connection_id: Option<String>,
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
    if request.github_connection_id.is_some() && request.github_operation.is_none() {
        return Err(ConnectionError::Invalid);
    }
    let observe_accounts = state.github_connection_approval
        && matches!(&request.target, ConnectionTarget::AgentServiceAccess { .. });
    let github = broker
        .github_connections(&caller.subject, observe_accounts)
        .await;
    let github_scope_check = if let Some(operation) = request.github_operation {
        let check = match request.github_connection_id.as_deref() {
            Some(id) => {
                broker
                    .exact_github_scope_check(&caller.subject, id, operation)
                    .await
            }
            None => github
                .as_ref()
                .map_or(GitHubScopeCheck::VerificationUnavailable, |github| {
                    github.scope_check(operation)
                }),
        };
        Some(GitHubScopeReport { operation, check })
    } else {
        None
    };
    Ok(no_store(RepositoryAccessReport {
        owner: caller.subject.to_string(),
        github,
        github_scope_check,
    }))
}
