use layer2_core::profiles::authoring::{
    AuthoringSourceError, RepositoryRequest, RepositoryResolution,
};
use serde::Deserialize;

use super::http::{invalid, json, unavailable};

pub(super) struct RepositoryResolver {
    pub http: reqwest::Client,
    pub origin: reqwest::Url,
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
    html_url: String,
    default_branch: String,
    private: bool,
    disabled: bool,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Deserialize)]
struct Reference {
    #[serde(rename = "ref")]
    name: String,
    object: GitObject,
}

#[derive(Deserialize)]
struct GitObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct Commit {
    sha: String,
}

#[derive(Deserialize)]
struct Tag {
    sha: String,
    object: GitObject,
}

/// Resolve public visibility and an exact revision using an anonymous client.
/// Confirm that the same fixed public archive endpoint is available before allocation.
pub(super) async fn resolve_public_snapshot(
    request: &RepositoryRequest,
) -> Result<RepositoryResolution, AuthoringSourceError> {
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("CMA-public-source/1.0")
        .build()
        .map_err(|_| unavailable("public repository client unavailable"))?;
    let resolver = RepositoryResolver {
        http: http.clone(),
        origin: reqwest::Url::parse("https://api.github.com").expect("fixed GitHub origin"),
    };
    let resolved = resolver.resolve_with_visibility(request, true).await?;
    let archive =
        layer3_adapter::public_repository_archive_url(&resolved.url, &resolved.commit_sha)
            .ok_or_else(|| invalid("public repository archive identity is invalid"))?;
    let response = http
        .head(archive)
        .send()
        .await
        .map_err(|_| unavailable("public archive verification failed"))?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(unavailable("public archive verification was refused"));
    }
    Ok(resolved)
}

impl RepositoryResolver {
    pub async fn resolve(
        &self,
        request: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        self.resolve_with_visibility(request, false).await
    }

    async fn resolve_with_visibility(
        &self,
        request: &RepositoryRequest,
        require_public_visibility: bool,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        let (owner, name) = repository_parts(&request.url)?;
        let response = self.get(&["repos", &owner, &name]).await?;
        let repository: Repository = json(response).await?;
        let expected_name = format!("{owner}/{name}");
        if repository.private
            || repository.disabled
            || (require_public_visibility && repository.visibility.as_deref() != Some("public"))
        {
            return Err(AuthoringSourceError::RepositoryNotFound);
        }
        if !repository.full_name.eq_ignore_ascii_case(&expected_name)
            || repository.html_url != format!("https://github.com/{}", repository.full_name)
            || repository.default_branch.is_empty()
            || repository.default_branch.len() > 255
        {
            return Err(invalid("repository response changed its identity"));
        }
        let (owner, name) = repository_parts(&repository.html_url)?;
        let reference = request
            .reference
            .as_deref()
            .unwrap_or(&repository.default_branch);
        let mut object =
            if require_public_visibility && request.reference.is_some() && valid_sha(reference) {
                let response = self
                    .get(&["repos", &owner, &name, "commits", reference])
                    .await?;
                let commit: Commit = json(response).await?;
                if commit.sha != reference {
                    return Err(invalid("repository commit response changed its identity"));
                }
                GitObject {
                    sha: commit.sha,
                    kind: "commit".into(),
                }
            } else {
                let branch = self.reference(&owner, &name, "heads", reference).await;
                match branch {
                    Err(AuthoringSourceError::ReferenceNotFound) if request.reference.is_some() => {
                        self.reference(&owner, &name, "tags", reference).await?
                    }
                    result => result?,
                }
            };
        // Annotated tags name tag objects, not commits. Resolve only the exact
        // observed object so a branch/tag moving during verification cannot
        // change the checked revision between requests.
        for _ in 0..8 {
            if object.kind == "commit" {
                break;
            }
            let tag: Tag = json(
                self.get(&["repos", &owner, &name, "git", "tags", &object.sha])
                    .await?,
            )
            .await?;
            if tag.sha != object.sha
                || !valid_sha(&tag.object.sha)
                || !matches!(tag.object.kind.as_str(), "commit" | "tag")
            {
                return Err(invalid("repository tag response changed its identity"));
            }
            object = tag.object;
        }
        if object.kind != "commit" || !valid_sha(&object.sha) {
            return Err(invalid("repository commit is not an exact object id"));
        }
        Ok(RepositoryResolution {
            url: repository.html_url,
            reference: request.reference.clone(),
            default_branch: repository.default_branch,
            commit_sha: object.sha,
        })
    }

    async fn reference(
        &self,
        owner: &str,
        name: &str,
        kind: &str,
        reference: &str,
    ) -> Result<GitObject, AuthoringSourceError> {
        let response = self
            .get(&["repos", owner, name, "git", "ref", kind, reference])
            .await
            .map_err(|error| match error {
                AuthoringSourceError::RepositoryNotFound => AuthoringSourceError::ReferenceNotFound,
                other => other,
            })?;
        let result: Reference = json(response).await?;
        if result.name != format!("refs/{kind}/{reference}")
            || !valid_sha(&result.object.sha)
            || !matches!(result.object.kind.as_str(), "commit" | "tag")
            || (kind == "heads" && result.object.kind != "commit")
        {
            return Err(invalid(
                "repository reference response changed its identity",
            ));
        }
        Ok(result.object)
    }

    async fn get(&self, parts: &[&str]) -> Result<reqwest::Response, AuthoringSourceError> {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|_| invalid("repository API origin has no path"))?
            .extend(parts);
        let response = self
            .http
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|_| unavailable("repository request failed"))?;
        match response.status().as_u16() {
            200 => Ok(response),
            404 => Err(AuthoringSourceError::RepositoryNotFound),
            _ => Err(unavailable("repository API refused verification")),
        }
    }
}

fn repository_parts(raw: &str) -> Result<(String, String), AuthoringSourceError> {
    let url = reqwest::Url::parse(raw).map_err(|_| invalid("repository URL is invalid"))?;
    let path = raw
        .strip_prefix("https://github.com/")
        .ok_or_else(|| invalid("repository URL is not a public GitHub locator"))?;
    let parts: Vec<_> = path.strip_suffix('/').unwrap_or(path).split('/').collect();
    if url.origin().ascii_serialization() != "https://github.com"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    {
        return Err(invalid("repository URL is not a public GitHub locator"));
    }
    let name = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    if name.is_empty() || matches!(name, "." | "..") || name.len() > 100 || parts[0].len() > 100 {
        return Err(invalid("repository identity exceeds its length bounds"));
    }
    Ok((parts[0].into(), name.into()))
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
