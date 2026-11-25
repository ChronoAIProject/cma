use axum::{
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse as _, Response},
    Json,
};
use layer3_adapter::{GithubCredentialRoute, GithubRepositoryScope, GithubRepositoryScopeNeed};
use serde_json::json;

pub(crate) const ACCESS_PATH: &str = "/.well-known/cma-github-access";
pub(crate) const SOURCE_AUTHORITY_HEADER: &str = "x-cma-github-source-authority";

pub(super) fn source_is_current(
    headers: &HeaderMap,
    route: Option<&GithubCredentialRoute>,
    generation: Option<u64>,
) -> bool {
    let Some(expected) = headers.get(SOURCE_AUTHORITY_HEADER) else {
        return true;
    };
    route.zip(generation).is_some_and(|(route, generation)| {
        expected.as_bytes() == format!("{}:{generation}", route.service_id).as_bytes()
    })
}

pub(super) fn operation_path<'a>(
    host: Option<&str>,
    model_proxy: bool,
    uri: &'a Uri,
    route: &GithubCredentialRoute,
) -> Option<&'a str> {
    match host {
        Some("api.github.com") => Some(uri.path()),
        None if !model_proxy => {
            let path = uri
                .path()
                .strip_prefix("/api/v1/proxy/")
                .or_else(|| uri.path().strip_prefix("/proxy/"))?;
            let (selector, _) = path.split_once('/')?;
            if selector == route.catalog_id || selector == route.service_id {
                return Some(&path[selector.len()..]);
            }
            path.strip_prefix("s/api-github")
                .filter(|remaining| remaining.starts_with('/'))
        }
        _ => None,
    }
}

pub(super) fn repository_metadata_uri(scope: &GithubRepositoryScope) -> Uri {
    format!("/repos/{}", scope.repository)
        .parse()
        .expect("validated GitHub repository path")
}

pub(super) fn metadata_request_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::ACCEPT,
        "application/vnd.github+json".parse().expect("static"),
    );
    headers.insert(header::ACCEPT_ENCODING, "identity".parse().expect("static"));
}

#[derive(serde::Deserialize)]
struct RepositoryMetadata {
    full_name: String,
    private: bool,
    disabled: bool,
    archived: bool,
    permissions: Option<RepositoryPermissions>,
}

#[derive(serde::Deserialize)]
struct RepositoryPermissions {
    push: bool,
}

impl RepositoryMetadata {
    fn permits(&self, scope: &GithubRepositoryScope) -> bool {
        self.full_name.eq_ignore_ascii_case(&scope.repository)
            && !self.disabled
            && (scope.required_scope != GithubRepositoryScopeNeed::PublicRepo
                || (!self.private
                    && !self.archived
                    && self
                        .permissions
                        .as_ref()
                        .is_some_and(|permissions| permissions.push)))
    }
}

async fn metadata(
    mut response: reqwest::Response,
    credential: &[u8],
) -> Option<RepositoryMetadata> {
    const MAX_BYTES: usize = 64 * 1024;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    if crate::upstream_guard::contains(&body, credential) {
        return None;
    }
    serde_json::from_slice(&body).ok()
}

pub(super) async fn access_report(
    response: reqwest::Response,
    route: &GithubCredentialRoute,
    scope: &GithubRepositoryScope,
    generation: u64,
    credential: &[u8],
) -> Response {
    let live = response
        .status()
        .is_success()
        .then(|| scopes(response.headers(), "x-oauth-scopes"))
        .flatten();
    let satisfied = live
        .as_ref()
        .is_some_and(|scopes| scope.required_scope.is_satisfied_by(scopes));
    let observed = live.map(|values| values.into_iter().map(str::to_owned).collect::<Vec<_>>());
    let scope_state = match &observed {
        Some(_) if satisfied => match metadata(response, credential).await {
            Some(repository) if repository.permits(scope) => "scope_satisfied",
            Some(_) => "repository_unavailable",
            None => "verification_unavailable",
        },
        Some(_) => "insufficient_scope",
        None => "verification_unavailable",
    };
    Json(json!({
        "service": "api-github",
        "connection_id": route.service_id,
        "credential_generation": generation,
        "repository": scope.repository,
        "required_scope": scope.required_scope,
        "status": scope_state,
        "granted_scopes": observed,
        "manage_url": scope.manage_url,
    }))
    .into_response()
}

pub(super) fn write_refusal(
    status: StatusCode,
    headers: &HeaderMap,
    path: &str,
    route: &GithubCredentialRoute,
    scope: &GithubRepositoryScope,
) -> Option<Response> {
    if scope.required_scope != GithubRepositoryScopeNeed::PublicRepo
        || !matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN)
        || one_header(headers, "x-github-request-id").is_none_or(str::is_empty)
        || headers.contains_key(header::RETRY_AFTER)
        || headers.contains_key("x-accepted-github-permissions")
        || (status == StatusCode::FORBIDDEN
            && one_header(headers, "x-ratelimit-remaining")
                .and_then(|remaining| remaining.parse::<u64>().ok())
                .is_none_or(|remaining| remaining == 0))
    {
        return None;
    }
    let prefix = format!("/repos/{}/", scope.repository);
    let endpoint = path.get(prefix.len()..).filter(|_| {
        path.get(..prefix.len())
            .is_some_and(|value| value.eq_ignore_ascii_case(&prefix))
    })?;
    if !matches!(endpoint, "git/refs" | "issues" | "pulls") {
        return None;
    }
    let accepted = scopes(headers, "x-accepted-oauth-scopes")?;
    let granted = scopes(headers, "x-oauth-scopes")?;
    if !matches!(
        accepted.as_slice(),
        ["public_repo"] | ["public_repo", "repo"] | ["repo", "public_repo"]
    ) || scope.required_scope.is_satisfied_by(&granted)
    {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "code": "insufficient_scope",
                "message": "GitHub connection requires public_repo for this repository write",
                "detail": "public_repo",
                "connection_id": route.service_id,
                "remediation_url": scope.manage_url,
            })),
        )
            .into_response(),
    )
}

fn one_header<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn scopes<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<Vec<&'a str>> {
    let value = one_header(headers, name)?.trim();
    if value.is_empty() {
        return Some(Vec::new());
    }
    let list: Vec<_> = value.split(',').map(str::trim).collect();
    (list.len() <= 100
        && list.iter().all(|scope| {
            (1..=64).contains(&scope.len())
                && scope
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
        }))
    .then_some(list)
}

pub(crate) async fn verify_repository_scope(
    client: &reqwest::Client,
    repository: &str,
    connection: &str,
) -> crate::failure::Result<String> {
    let response = client
        .get(format!("http://api.github.com{}", ACCESS_PATH))
        .send()
        .await
        .map_err(|_| {
            crate::failure::RuntimeError::unavailable(
                "repository_access_unavailable",
                "GitHub access could not be checked",
            )
        })?;
    let report: serde_json::Value = response.json().await.map_err(|_| {
        crate::failure::RuntimeError::unavailable(
            "repository_access_unavailable",
            "GitHub access evidence was invalid",
        )
    })?;
    if report["connection_id"] == connection
        && report["repository"]
            .as_str()
            .is_some_and(|bound| bound.eq_ignore_ascii_case(repository))
        && report["status"] == "scope_satisfied"
    {
        if let Some(generation) = report["credential_generation"]
            .as_u64()
            .filter(|generation| *generation > 0)
        {
            return Ok(format!("{connection}:{generation}"));
        }
    }
    Err(crate::failure::RuntimeError::rejected(
        "repository_connection_required",
        "The selected GitHub connection must verify access before source preparation",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> GithubCredentialRoute {
        GithubCredentialRoute {
            service_id: "selected".into(),
            catalog_id: "github-catalog".into(),
            other_services: vec![],
            repository_scope: Some(GithubRepositoryScope {
                repository: "example/project".into(),
                required_scope: GithubRepositoryScopeNeed::PublicRepo,
                manage_url: "https://nyx.example/keys/selected".into(),
            }),
        }
    }

    #[tokio::test]
    async fn scope_refusal_requires_exact_scope_and_preserves_independent_provider_failures() {
        let route = route();
        let scope = route.repository_scope.as_ref().unwrap();
        let path = "/repos/example/project/git/refs";
        let mut headers = HeaderMap::new();
        headers.insert("x-github-request-id", "fixture".parse().unwrap());
        assert!(write_refusal(StatusCode::NOT_FOUND, &headers, path, &route, scope).is_none());
        headers.insert("x-accepted-oauth-scopes", "public_repo".parse().unwrap());
        headers.insert("x-oauth-scopes", "read:user".parse().unwrap());
        assert!(write_refusal(
            StatusCode::NOT_FOUND,
            &headers,
            "/repos/example/other/git/refs",
            &route,
            scope
        )
        .is_none());
        let response = write_refusal(StatusCode::NOT_FOUND, &headers, path, &route, scope).unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "insufficient_scope");
        assert_eq!(body["detail"], "public_repo");
        assert_eq!(body["remediation_url"], "https://nyx.example/keys/selected");
        for alternatives in ["public_repo, repo", "repo, public_repo"] {
            headers.insert("x-accepted-oauth-scopes", alternatives.parse().unwrap());
            assert!(write_refusal(StatusCode::NOT_FOUND, &headers, path, &route, scope).is_some());
        }
        for ambiguous in ["public_repo, public_repo", "public_repo, repo, admin:org"] {
            headers.insert("x-accepted-oauth-scopes", ambiguous.parse().unwrap());
            assert!(write_refusal(StatusCode::NOT_FOUND, &headers, path, &route, scope).is_none());
        }
        headers.insert(
            "x-accepted-oauth-scopes",
            "public_repo, repo".parse().unwrap(),
        );
        for present in ["repo", "public_repo"] {
            headers.insert("x-oauth-scopes", present.parse().unwrap());
            assert!(write_refusal(StatusCode::NOT_FOUND, &headers, path, &route, scope).is_none());
        }
        headers.insert("x-oauth-scopes", "read:user".parse().unwrap());
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        assert!(write_refusal(StatusCode::FORBIDDEN, &headers, path, &route, scope).is_none());
        headers.insert("x-ratelimit-remaining", "10".parse().unwrap());
        headers.insert(header::RETRY_AFTER, "30".parse().unwrap());
        assert!(write_refusal(StatusCode::FORBIDDEN, &headers, path, &route, scope).is_none());
        headers.remove(header::RETRY_AFTER);
        headers.append("x-oauth-scopes", "repo".parse().unwrap());
        assert!(write_refusal(StatusCode::NOT_FOUND, &headers, path, &route, scope).is_none());
    }

    #[test]
    fn an_admitted_source_read_refuses_a_rotated_credential_or_replaced_connection() {
        let mut route = route();
        let mut headers = HeaderMap::new();
        headers.insert(SOURCE_AUTHORITY_HEADER, "selected:4".parse().unwrap());
        assert!(source_is_current(&headers, Some(&route), Some(4)));
        assert!(!source_is_current(&headers, Some(&route), Some(5)));
        assert!(!source_is_current(&headers, None, Some(4)));

        route.service_id = "replacement".into();
        assert!(!source_is_current(&headers, Some(&route), Some(4)));
        headers.remove(SOURCE_AUTHORITY_HEADER);
        assert!(source_is_current(&headers, Some(&route), Some(4)));
    }

    #[test]
    fn platform_scope_refusal_is_limited_to_exact_github_paths() {
        let route = route();
        for (request, expected) in [
            (
                "/api/v1/proxy/s/api-github/repos/example/project/issues",
                Some("/repos/example/project/issues"),
            ),
            ("/proxy/github-catalog/user", Some("/user")),
            ("/api/v1/proxy/selected/user", Some("/user")),
            ("/api/v1/proxy/s/api-github-shadow/user", None),
            ("/api/v1/proxy/s/ornn-api/skills", None),
        ] {
            assert_eq!(
                operation_path(None, false, &request.parse().unwrap(), &route),
                expected
            );
        }
    }
}
