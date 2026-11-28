use super::*;

#[derive(Serialize)]
pub(super) struct GitHubAccount {
    login: String,
    observed_scopes: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct GitHubUser {
    id: u64,
    login: String,
}

impl NyxidServiceConnectionBroker {
    pub(super) async fn github_account(
        &self,
        credential: &SecretValue,
        catalog_id: &str,
        connection_id: &str,
        deadline: Instant,
    ) -> Option<GitHubAccount> {
        // The owner can still see the inventory when the optional GitHub read
        // fails. A single provider deadline bounds the entire report.
        let deadline = deadline.min(Instant::now() + Duration::from_secs(1));
        timeout_at(deadline, async {
            let response = self
                .github_user(credential, catalog_id, connection_id, deadline)
                .await
                .ok()?;
            let observed_scopes = observed_oauth_scopes(response.headers())
                .ok()
                .map(|scopes| scopes.into_iter().map(str::to_owned).collect());
            let user: GitHubUser = decode(response).await.ok()?;
            (user.id > 0
                && !user.login.is_empty()
                && user.login.len() <= 100
                && user
                    .login
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
            .then_some(GitHubAccount {
                login: user.login,
                observed_scopes,
            })
        })
        .await
        .ok()
        .flatten()
    }
}
