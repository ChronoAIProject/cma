use serde::Serialize;

use super::*;

pub(crate) const GITHUB_SERVICE: &str = "api-github";
const MAX_CHOICES: usize = 12;

pub(crate) struct VerifiedGitHubConnection {
    connection_id: String,
    slug: String,
}

impl VerifiedGitHubConnection {
    pub(crate) fn id(&self) -> &str {
        &self.connection_id
    }

    pub(crate) fn slug(&self) -> &str {
        &self.slug
    }
}

#[derive(Serialize)]
pub(crate) struct GitHubConnections {
    manage_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    connections: Option<Vec<GitHubConnection>>,
}

#[derive(Serialize)]
struct GitHubConnection {
    id: String,
    label: String,
    granted_scopes: Option<Vec<String>>,
    manage_url: String,
}

#[derive(Deserialize)]
struct Catalog {
    services: Vec<CatalogService>,
}

#[derive(Deserialize)]
struct CatalogService {
    id: String,
    slug: String,
}

#[derive(Deserialize)]
struct KeyDetail {
    id: String,
    slug: String,
    catalog_service_id: Option<String>,
    label: String,
    granted_scopes: Option<Vec<String>>,
}

impl NyxidServiceConnectionBroker {
    pub(crate) async fn github_connections(&self, owner: &SubjectId) -> GitHubConnections {
        let manage_url = self
            .browser_origin
            .join("/keys")
            .expect("valid HTTPS origin");
        let deadline = Instant::now() + PROVIDER_HTTP_TIMEOUT;
        let credential = self.credential(owner).await;
        let connections = match credential {
            Ok(credential) if Instant::now() < deadline => {
                timeout_at(deadline, self.github_inventory(&credential, deadline))
                    .await
                    .ok()
            }
            _ => None,
        };
        let connections = match connections {
            Some(Ok(connections)) => Some(connections),
            _ => {
                tracing::debug!("Owner GitHub connection inventory could not be verified");
                None
            }
        };
        GitHubConnections {
            manage_url: manage_url.to_string(),
            connections,
        }
    }

    async fn github_inventory(
        &self,
        credential: &SecretValue,
        deadline: Instant,
    ) -> Result<Vec<GitHubConnection>, ServiceConnectionError> {
        let (catalog_id, rows) = self.github_source(credential, deadline).await?;
        let catalog_id = &catalog_id;
        let choices = rows
            .iter()
            .filter(|row| row.is_allowed() && row.catalog_service_id.as_ref() == Some(catalog_id))
            .collect::<Vec<_>>();
        if choices.len() > MAX_CHOICES
            || choices.iter().any(|row| {
                !valid_browser_id(&row.id)
                    || rows.iter().filter(|other| other.id == row.id).count() != 1
            })
        {
            return Err(malformed());
        }
        let choices = choices
            .into_iter()
            .map(|row| (row.id.clone(), row.slug.clone()))
            .collect::<Vec<_>>();
        let results = futures_util::stream::iter(choices)
            .map(|(id, slug)| async move {
                self.github_detail(&id, &slug, catalog_id, credential, deadline)
                    .await
            })
            .buffered(4)
            .collect::<Vec<_>>()
            .await;
        let mut connections = results.into_iter().collect::<Result<Vec<_>, _>>()?;
        connections.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(connections)
    }

    async fn github_source(
        &self,
        credential: &SecretValue,
        deadline: Instant,
    ) -> Result<(String, Vec<UserServiceRow>), ServiceConnectionError> {
        let catalog = async {
            let response = self
                .send(
                    self.http
                        .get(self.endpoint("/api/v1/services"))
                        .bearer_auth(credential.expose()),
                    false,
                    Some(deadline),
                )
                .await?;
            decode::<Catalog>(response).await
        };
        let (catalog, rows) =
            tokio::try_join!(catalog, self.user_services(credential, Some(deadline)))?;
        let mut github = catalog
            .services
            .into_iter()
            .filter(|row| row.slug == GITHUB_SERVICE);
        let catalog_id = github.next().ok_or_else(malformed)?.id;
        if !valid_provider_id(&catalog_id) || github.next().is_some() {
            return Err(malformed());
        }
        Ok((catalog_id, rows))
    }

    #[cfg(test)]
    pub(in crate::trigger) async fn verify_github_repo_connection(
        &self,
        owner: &SubjectId,
        connection_id: &str,
    ) -> Result<VerifiedGitHubConnection, ServiceConnectionError> {
        let deadline = Instant::now() + PROVIDER_HTTP_TIMEOUT;
        timeout_at(deadline, async {
            let credential = self.credential(owner).await?;
            self.verify_github_repo_token(&credential, connection_id, deadline)
                .await
        })
        .await
        .map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?
    }

    pub(super) async fn verify_github_repo_token(
        &self,
        credential: &SecretValue,
        connection_id: &str,
        deadline: Instant,
    ) -> Result<VerifiedGitHubConnection, ServiceConnectionError> {
        if !valid_browser_id(connection_id) {
            return Err(ServiceConnectionError::Refused);
        }
        timeout_at(deadline, async {
            let (catalog_id, rows) = self.github_source(credential, deadline).await?;
            let row = exact_github_row(connection_id, &catalog_id, &rows)?;
            let detail = self
                .github_detail(connection_id, &row.slug, &catalog_id, credential, deadline)
                .await?;
            if !valid_browser_id(&catalog_id)
                || !detail
                    .granted_scopes
                    .is_some_and(|scopes| scopes.iter().any(|scope| scope == "repo"))
            {
                return Err(ServiceConnectionError::Refused);
            }
            let response = self
                .send(
                    self.http
                        .get(self.endpoint(&format!("/api/v1/proxy/{catalog_id}/user")))
                        .query(&[("_nyxid_via", connection_id)])
                        .bearer_auth(credential.expose()),
                    true,
                    Some(deadline),
                )
                .await?;
            let scopes = response
                .headers()
                .get("x-oauth-scopes")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| unavailable("selected GitHub credential scope was not observed"))?;
            if !scopes.split(',').any(|scope| scope.trim() == "repo") {
                return Err(ServiceConnectionError::Refused);
            }
            Ok(VerifiedGitHubConnection {
                connection_id: detail.id,
                slug: row.slug.clone(),
            })
        })
        .await
        .map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?
    }

    async fn github_detail(
        &self,
        id: &str,
        slug: &str,
        catalog_id: &str,
        credential: &SecretValue,
        deadline: Instant,
    ) -> Result<GitHubConnection, ServiceConnectionError> {
        // NyxID's owner-visible inventory supplies the candidate. The detail
        // must still agree on exact row and catalog, including for custom slugs.
        let response = self
            .send(
                self.http
                    .get(self.endpoint(&format!("/api/v1/keys/{id}")))
                    .bearer_auth(credential.expose()),
                true,
                Some(deadline),
            )
            .await?;
        let detail: KeyDetail = decode(response).await?;
        if detail.id != id
            || detail.slug != slug
            || detail.catalog_service_id.as_deref() != Some(catalog_id)
            || !valid_provider_id(&detail.label)
            || detail.granted_scopes.as_ref().is_some_and(|scopes| {
                scopes.len() > 100 || scopes.iter().any(|scope| !valid_provider_id(scope))
            })
        {
            return Err(malformed());
        }
        let manage_url = self
            .browser_origin
            .join(&format!("/keys/{}", detail.id))
            .expect("validated connection ID");
        Ok(GitHubConnection {
            id: detail.id,
            label: detail.label,
            granted_scopes: detail.granted_scopes,
            manage_url: manage_url.to_string(),
        })
    }
}

fn valid_browser_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn exact_github_row<'a>(
    connection_id: &str,
    catalog_id: &str,
    rows: &'a [UserServiceRow],
) -> Result<&'a UserServiceRow, ServiceConnectionError> {
    let mut exact = rows.iter().filter(|row| row.id == connection_id);
    let row = exact.next().ok_or(ServiceConnectionError::Refused)?;
    if exact.next().is_some()
        || !row.is_usable()
        || row.catalog_service_id.as_deref() != Some(catalog_id)
    {
        return Err(ServiceConnectionError::Refused);
    }
    Ok(row)
}
