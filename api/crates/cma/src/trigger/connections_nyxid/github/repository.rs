use super::*;
use layer1_public::rest::AgentLifecycleRepository;

pub(crate) struct VerifiedGitHubRepository {
    pub(crate) connection_id: String,
    pub(crate) commit: String,
    pub(crate) repository: String,
}

pub(crate) enum GitHubRepositoryCheck {
    Verified(VerifiedGitHubRepository),
    Scope(GitHubScopeCheck),
    RepositoryUnavailable,
    ReferenceUnavailable,
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
    default_branch: String,
    private: bool,
    disabled: bool,
    archived: bool,
    permissions: Option<RepositoryPermissions>,
}

#[derive(Deserialize)]
struct RepositoryPermissions {
    push: bool,
}

#[derive(Deserialize)]
struct Commit {
    sha: String,
}

impl NyxidServiceConnectionBroker {
    /// Check the provider's live OAuth observation for an explicitly requested
    /// owner connection. An inventory and detail disagreement stays unknown.
    pub(in crate::trigger) async fn exact_github_scope_check(
        &self,
        owner: &SubjectId,
        connection_id: &str,
        need: GitHubRepositoryNeed,
    ) -> GitHubScopeCheck {
        if !valid_browser_id(connection_id) {
            return GitHubScopeCheck::VerificationUnavailable;
        }
        let deadline = Instant::now() + PROVIDER_HTTP_TIMEOUT;
        timeout_at(deadline, async {
            let credential = self.credential(owner).await?;
            let (catalog_id, rows) = self.github_source(&credential, deadline).await?;
            let row = exact_github_row(connection_id, &catalog_id, &rows)?;
            let detail = self
                .github_detail(connection_id, &row.slug, &catalog_id, &credential, deadline)
                .await?;
            self.observe_scope(&credential, &catalog_id, &detail, need, deadline)
                .await
        })
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(GitHubScopeCheck::VerificationUnavailable)
    }

    pub(super) async fn observe_scope(
        &self,
        credential: &SecretValue,
        catalog_id: &str,
        detail: &GitHubConnection,
        need: GitHubRepositoryNeed,
        deadline: Instant,
    ) -> Result<GitHubScopeCheck, ServiceConnectionError> {
        let recorded = detail
            .granted_scopes
            .as_ref()
            .filter(|scopes| {
                detail.active_oauth
                    && valid_browser_id(catalog_id)
                    && scopes.iter().all(|scope| valid_git_scope(scope))
            })
            .ok_or_else(malformed)?;
        let live = self
            .github_user(credential, catalog_id, &detail.id, deadline)
            .await?;
        let mut values = live.headers().get_all("x-oauth-scopes").iter();
        let header = values
            .next()
            .filter(|_| values.next().is_none())
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| unavailable("selected GitHub credential scope was not observed"))?;
        let scopes = header.trim().split(',').map(str::trim);
        if !header.trim().is_empty()
            && (scopes.clone().count() > 100 || !scopes.clone().all(valid_git_scope))
        {
            return Err(malformed());
        }
        let satisfied = scope_satisfied(need, scopes);
        if satisfied != scope_satisfied(need, recorded.iter().map(String::as_str)) {
            return Err(malformed());
        }
        Ok::<_, ServiceConnectionError>(if satisfied {
            GitHubScopeCheck::ScopeSatisfied {
                connection_id: detail.id.clone(),
            }
        } else {
            GitHubScopeCheck::InsufficientScope {
                connection_id: detail.id.clone(),
                required_scope: required_scope(need),
                manage_url: detail.manage_url.clone(),
            }
        })
    }

    pub(in crate::trigger) async fn verify_github_repository(
        &self,
        owner: &SubjectId,
        repository: &AgentLifecycleRepository,
        retained_connection: Option<&str>,
        pinned_commit: Option<&str>,
    ) -> GitHubRepositoryCheck {
        let deadline = Instant::now() + PROVIDER_HTTP_TIMEOUT;
        timeout_at(deadline, async {
            let credential = self.credential(owner).await?;
            let (catalog_id, rows) = self.github_source(&credential, deadline).await?;
            let id = match retained_connection.or(repository.connection_id.as_deref()) {
                Some(id) if valid_browser_id(id) => id.to_owned(),
                Some(_) => return Err(malformed()),
                None => match default_binding(&rows, &catalog_id) {
                    Binding::Exact(id) if valid_browser_id(&id) => id,
                    Binding::Missing => {
                        return Ok(GitHubRepositoryCheck::Scope(
                            GitHubScopeCheck::MissingConnection,
                        ));
                    }
                    _ => return Err(malformed()),
                },
            };
            let row = exact_github_row(&id, &catalog_id, &rows)?;
            let detail = self
                .github_detail(&id, &row.slug, &catalog_id, &credential, deadline)
                .await?;
            let need = repository
                .operation
                .unwrap_or(GitHubRepositoryNeed::PrivateRepositoryRead);
            let scope = self
                .observe_scope(&credential, &catalog_id, &detail, need, deadline)
                .await?;
            if !matches!(scope, GitHubScopeCheck::ScopeSatisfied { .. }) {
                return Ok(GitHubRepositoryCheck::Scope(scope));
            }
            let [namespace, name] = repository_path(&repository.url).ok_or_else(malformed)?;
            let get = |path: reqwest::Url| {
                self.send(
                    self.http
                        .get(path)
                        .query(&[("_nyxid_via", &id)])
                        .bearer_auth(credential.expose())
                        .header("Accept", "application/vnd.github+json")
                        .header("X-GitHub-Api-Version", "2022-11-28"),
                    true,
                    Some(deadline),
                )
            };
            let mut url =
                reqwest::Url::parse(&self.endpoint(&format!("/api/v1/proxy/{catalog_id}/repos")))
                    .map_err(|_| malformed())?;
            url.path_segments_mut()
                .map_err(|_| malformed())?
                .extend([namespace, name]);
            let metadata: Repository = match get(url.clone()).await {
                Ok(response) if response_has_scope(response.headers(), need) => {
                    decode(response).await?
                }
                Ok(_) => return Err(malformed()),
                Err(ServiceConnectionError::NotFound | ServiceConnectionError::Refused) => {
                    return Ok(GitHubRepositoryCheck::RepositoryUnavailable);
                }
                Err(error) => return Err(error),
            };
            if !metadata
                .full_name
                .eq_ignore_ascii_case(&format!("{namespace}/{name}"))
                || metadata.disabled
                || (need == GitHubRepositoryNeed::PublicRepositoryWrite
                    && (metadata.private
                        || metadata.archived
                        || !metadata.permissions.is_some_and(|value| value.push)))
            {
                return Ok(GitHubRepositoryCheck::RepositoryUnavailable);
            }
            let reference = pinned_commit
                .or(repository.reference.as_deref())
                .unwrap_or(&metadata.default_branch);
            if reference.is_empty() || reference.len() > 255 {
                return Err(malformed());
            }
            url.path_segments_mut()
                .map_err(|_| malformed())?
                .extend(["commits", reference]);
            let commit: Commit = match get(url).await {
                Ok(response) if response_has_scope(response.headers(), need) => {
                    decode(response).await?
                }
                Ok(_) => return Err(malformed()),
                Err(ServiceConnectionError::NotFound | ServiceConnectionError::Refused) => {
                    return Ok(GitHubRepositoryCheck::ReferenceUnavailable);
                }
                Err(error) => return Err(error),
            };
            if !matches!(commit.sha.len(), 40 | 64)
                || !commit.sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                || pinned_commit.is_some_and(|pin| !commit.sha.eq_ignore_ascii_case(pin))
            {
                return Err(malformed());
            }
            Ok(GitHubRepositoryCheck::Verified(VerifiedGitHubRepository {
                connection_id: id,
                commit: commit.sha.to_ascii_lowercase(),
                repository: format!("{namespace}/{name}"),
            }))
        })
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(GitHubRepositoryCheck::Scope(
            GitHubScopeCheck::VerificationUnavailable,
        ))
    }
}

fn repository_path(raw: &str) -> Option<[&str; 2]> {
    let path = raw
        .strip_prefix("https://github.com/")?
        .trim_end_matches('/');
    let (namespace, name) = path.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    [namespace, name]
        .iter()
        .all(|part| {
            !part.is_empty()
                && part.len() <= 100
                && !matches!(*part, "." | "..")
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
        .then_some([namespace, name])
}

fn response_has_scope(headers: &reqwest::header::HeaderMap, need: GitHubRepositoryNeed) -> bool {
    let mut values = headers.get_all("x-oauth-scopes").iter();
    values
        .next()
        .filter(|_| values.next().is_none())
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            let scopes = value.split(',').map(str::trim);
            scopes.clone().count() <= 100
                && scopes.clone().all(valid_git_scope)
                && scope_satisfied(need, scopes)
        })
}
