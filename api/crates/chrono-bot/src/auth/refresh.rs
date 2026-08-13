//! Exchanging a refresh token for a new access token.
//!
//! Separated so the module that holds tokens stays about custody and this one
//! is only about renewing them.

use serde::Deserialize;

use layer2_core::SubjectId;

use super::{AuthMode, AuthState, SubjectTokens};

impl AuthState {
    /// Exchange a refresh token for a new access token.
    ///
    /// A provider that refuses the exchange has withdrawn the delegation, so
    /// the stored tokens are dropped rather than retried forever.
    pub async fn refresh(
        &self,
        subject: &SubjectId,
        refresh_token: &str,
    ) -> Result<String, String> {
        let AuthMode::Provider {
            client_id,
            client_secret,
            ..
        } = &self.mode
        else {
            return Err("no provider configured".into());
        };
        let meta = self.metadata().await?;
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];
        let resp = self
            .http
            .post(&meta.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("refresh request failed: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            self.tokens.write().await.remove(subject);
            return Err(format!("provider refused refresh with {status}"));
        }
        #[derive(Deserialize)]
        struct RefreshResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            expires_in: Option<u64>,
        }
        let body: RefreshResponse = resp
            .json()
            .await
            .map_err(|e| format!("refresh response not usable: {e}"))?;

        let mut store = self.tokens.write().await;
        let existing_refresh = store.get(subject).and_then(|t| t.refresh_token.clone());
        store.insert(
            subject.clone(),
            SubjectTokens {
                access_token: body.access_token.clone(),
                // A provider that rotates refresh tokens returns a new one; one
                // that does not expects the original to keep working.
                refresh_token: body.refresh_token.or(existing_refresh),
                expires_at: std::time::Instant::now()
                    + std::time::Duration::from_secs(body.expires_in.unwrap_or(3600)),
            },
        );
        Ok(body.access_token)
    }
}
