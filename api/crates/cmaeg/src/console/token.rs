//! The NyxID token and revocation endpoints as the console uses them: a
//! confidential client authenticating with `client_secret_post`. Tokens,
//! codes and verifiers are held as [`Secret`] and never logged; upstream
//! bodies are never echoed.
use std::time::Duration;

use cmaeg_core::channel::Secret;
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;

const TOKEN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY_BYTES: usize = 64 * 1024;
/// Longer than any token NyxID issues; a longer value is not a token.
const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// A token response the console can use.
pub struct TokenSet {
    pub access: Secret,
    /// NyxID always issues one, and rotates it on every refresh.
    pub refresh: Secret,
    pub expires_in: Duration,
}

/// How a failed token request is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFailure {
    /// NyxID refused the grant (`invalid_grant`, 401, or another 4xx
    /// that retrying cannot fix): the session it belongs to ends.
    Definitive,
    /// The answer may differ later: a timeout, a connection error, 429, 5xx
    /// or a malformed body. The session is kept.
    Transient,
}

#[derive(Clone)]
pub struct TokenClient {
    http: Client,
    client_id: String,
    client_secret: Secret,
}

impl TokenClient {
    pub fn new(http: Client, client_id: &str, client_secret: &Secret) -> Self {
        Self {
            http,
            client_id: client_id.to_owned(),
            client_secret: client_secret.clone(),
        }
    }

    // NyxID's authorization-code handler requires form credentials even though
    // discovery also advertises Basic. Use one authentication method, never both.
    fn post(&self, endpoint: &Url, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let mut form = fields.to_vec();
        form.push(("client_id", &self.client_id));
        form.push(("client_secret", self.client_secret.as_str()));
        self.http
            .post(endpoint.clone())
            .form(&form)
            .timeout(TOKEN_TIMEOUT)
    }

    /// Redeem an authorization code with its PKCE verifier.
    pub async fn exchange(
        &self,
        endpoint: &Url,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenSet, TokenFailure> {
        self.request(
            endpoint,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
            ],
        )
        .await
    }

    /// Rotate a refresh token. The caller must replace the old token with the
    /// returned one before any other refresh, because NyxID revokes the whole
    /// grant when a rotated token is reused.
    pub async fn refresh(
        &self,
        endpoint: &Url,
        refresh: &Secret,
    ) -> Result<TokenSet, TokenFailure> {
        self.request(
            endpoint,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
            ],
        )
        .await
    }

    /// Best-effort RFC 7009 revocation; true when NyxID answered 200.
    pub async fn revoke(&self, endpoint: &Url, token: &Secret) -> bool {
        self.post(
            endpoint,
            &[
                ("token", token.as_str()),
                ("token_type_hint", "refresh_token"),
            ],
        )
        .send()
        .await
        .is_ok_and(|response| response.status() == StatusCode::OK)
    }

    async fn request(
        &self,
        endpoint: &Url,
        form: &[(&str, &str)],
    ) -> Result<TokenSet, TokenFailure> {
        let mut response = self
            .post(endpoint, form)
            .send()
            .await
            .map_err(|_| TokenFailure::Transient)?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| TokenFailure::Transient)?
        {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(TokenFailure::Transient);
            }
            body.extend_from_slice(&chunk);
        }
        classify(status, &body)
    }
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
}

/// The classification table of the token endpoint (Amendment R6 §R6.4).
pub(crate) fn classify(status: StatusCode, body: &[u8]) -> Result<TokenSet, TokenFailure> {
    if status == StatusCode::OK {
        let parsed: TokenBody =
            serde_json::from_slice(body).map_err(|_| TokenFailure::Transient)?;
        let refresh = parsed.refresh_token.ok_or(TokenFailure::Transient)?;
        if !usable(&parsed.access_token) || !usable(&refresh) {
            return Err(TokenFailure::Transient);
        }
        return Ok(TokenSet {
            access: Secret::new(parsed.access_token),
            refresh: Secret::new(refresh),
            // NyxID's default; a response without it is treated as that.
            expires_in: Duration::from_secs(parsed.expires_in.unwrap_or(900).clamp(1, 86_400)),
        });
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Err(TokenFailure::Transient);
    }
    if status == StatusCode::BAD_REQUEST {
        let error = serde_json::from_slice::<ErrorBody>(body)
            .ok()
            .and_then(|body| body.error);
        return match error.as_deref() {
            // An unparsable 400 is not NyxID's OAuth answer; a proxy in
            // between may produce it, so it is retried.
            None => Err(TokenFailure::Transient),
            Some(_) => Err(TokenFailure::Definitive),
        };
    }
    if status.is_client_error() {
        return Err(TokenFailure::Definitive);
    }
    Err(TokenFailure::Transient)
}

fn usable(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_TOKEN_BYTES
        && token.bytes().all(|byte| (33..=126).contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_body(refresh: Option<&str>) -> Vec<u8> {
        serde_json::json!({"access_token": "a1", "refresh_token": refresh, "expires_in": 900})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn the_token_endpoint_results_are_classified_as_the_table_says() {
        let rotated = classify(StatusCode::OK, &token_body(Some("r2"))).unwrap();
        assert_eq!(rotated.access.as_str(), "a1");
        assert_eq!(rotated.refresh.as_str(), "r2");
        assert_eq!(rotated.expires_in, Duration::from_secs(900));
        let definitive = [
            (StatusCode::BAD_REQUEST, r#"{"error":"invalid_grant"}"#),
            (StatusCode::BAD_REQUEST, r#"{"error":"invalid_request"}"#),
            (StatusCode::UNAUTHORIZED, ""),
            (StatusCode::FORBIDDEN, "{}"),
        ];
        for (status, body) in definitive {
            assert_eq!(
                classify(status, body.as_bytes()).err(),
                Some(TokenFailure::Definitive),
                "{status} {body}"
            );
        }
        let transient = [
            (StatusCode::TOO_MANY_REQUESTS, "{}"),
            (StatusCode::SERVICE_UNAVAILABLE, ""),
            (StatusCode::BAD_GATEWAY, "<html>"),
            (StatusCode::BAD_REQUEST, "<html>proxy</html>"),
            (StatusCode::OK, "not json"),
            (StatusCode::OK, r#"{"access_token":"a1"}"#),
            (StatusCode::OK, r#"{"access_token":"","refresh_token":"r"}"#),
            (
                StatusCode::OK,
                r#"{"access_token":"a b","refresh_token":"r"}"#,
            ),
        ];
        for (status, body) in transient {
            assert_eq!(
                classify(status, body.as_bytes()).err(),
                Some(TokenFailure::Transient),
                "{status} {body}"
            );
        }
        assert!(classify(StatusCode::OK, &token_body(None)).is_err());
    }

    #[test]
    fn client_credentials_use_only_correctly_encoded_form_fields() {
        let client = TokenClient::new(Client::new(), "id:x", &Secret::new("p&ss=word+"));
        let request = client
            .post(
                &Url::parse("https://nyx.example/oauth/token").unwrap(),
                &[("grant_type", "authorization_code")],
            )
            .build()
            .unwrap();
        assert!(!request
            .headers()
            .contains_key(reqwest::header::AUTHORIZATION));
        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            b"grant_type=authorization_code&client_id=id%3Ax&client_secret=p%26ss%3Dword%2B"
        );
    }
}
