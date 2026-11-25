//! Existing owner-credential repository source transport.
use super::*;

pub(super) async fn archive(
    config: &RuntimeConfig,
    url: &str,
    reference: &Option<String>,
    connection_id: Option<&str>,
    pinned_commit: Option<&str>,
    operation_ref: &str,
) -> Result<(String, reqwest::RequestBuilder)> {
    let proxy = config.github_proxy.as_ref().ok_or_else(|| {
        observe_failure(
            operation_ref,
            ReadStage::ProxySetup,
            None,
            ReadFailure::ProxyMissing,
            ResponseHint::Unknown,
        );
        RuntimeError::rejected(
            "repository_connection_required",
            "GitHub connection is required before repository preparation",
        )
    })?;
    let client = reqwest::Client::builder()
        .unix_socket(proxy.socket())
        .no_proxy()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| transport_failure(operation_ref, ReadStage::ProxySetup, None, &error))?;
    let parsed = url::Url::parse(url).map_err(|_| invalid("invalid repository URL"))?;
    let repository = parsed.path().trim_matches('/').trim_end_matches(".git");
    let authority = match connection_id {
        Some(connection) => Some(
            crate::upstream::github_scope::verify_repository_scope(&client, repository, connection)
                .await?,
        ),
        None => None,
    };
    let source_read = |url: &str| {
        let request = client.get(url);
        match &authority {
            Some(authority) => request.header(
                crate::upstream::github_scope::SOURCE_AUTHORITY_HEADER,
                authority,
            ),
            None => request,
        }
    };
    let endpoint = format!("http://api.github.com/repos/{repository}");
    let source_commit = if let Some(commit) = pinned_commit {
        commit.to_owned()
    } else {
        let bytes = fetch(
            source_read(&endpoint),
            "repository_unavailable",
            operation_ref,
            ReadStage::Metadata,
        )
        .await?;
        let metadata: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid("invalid repository metadata"))?;
        let reference = match reference {
            Some(reference) => reference.clone(),
            None => metadata["default_branch"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid("repository has no default branch"))?
                .to_owned(),
        };
        let commits = fetch(
            source_read(&format!("{endpoint}/commits"))
                .query(&[("sha", reference.as_str()), ("per_page", "1")]),
            "repository_ref_not_found",
            operation_ref,
            ReadStage::Commits,
        )
        .await?;
        let commits: serde_json::Value = serde_json::from_slice(&commits)
            .map_err(|_| invalid("invalid repository commit response"))?;
        commits[0]["sha"]
            .as_str()
            .filter(|value| valid_commit(value))
            .ok_or_else(|| invalid("repository ref did not resolve to a commit"))?
            .to_owned()
    };
    if connection_id.is_some() && reference.as_ref() != Some(&source_commit) {
        return Err(invalid(
            "repository commit does not match the admitted source",
        ));
    }
    let archive = source_read(&format!("{endpoint}/zipball/{source_commit}"));
    Ok((source_commit, archive))
}
