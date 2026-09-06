//! On-demand renewal of encrypted provider tokens.

use std::time::{Duration, SystemTime};

use layer2_core::StoreError;
use serde::Deserialize;

use super::{
    persistence::{DurableSubjectTokens, SecretValue, SubjectAuthState},
    AuthMode, AuthState,
};

const REFRESH_MARGIN: Duration = Duration::from_secs(60);
const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
}

pub(super) async fn refresh_if_needed(
    auth: &AuthState,
    state: &mut SubjectAuthState,
) -> Result<(), StoreError> {
    let Some(tokens) = state.tokens.as_ref() else {
        return Ok(());
    };
    let refresh_at = tokens
        .expires_at
        .checked_sub(REFRESH_MARGIN)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if SystemTime::now() < refresh_at {
        return Ok(());
    }
    let Some(refresh_token) = tokens.refresh_token.as_ref() else {
        state.tokens = None;
        auth.store.put_subject(state.clone()).await?;
        return Ok(());
    };
    let AuthMode::Provider {
        client_id,
        client_secret,
        ..
    } = &auth.mode
    else {
        return Ok(());
    };
    let metadata = auth.metadata().await.map_err(StoreError::Unavailable)?;
    let response = auth
        .http
        .post(&metadata.token_endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(|error| StoreError::Unavailable(format!("refresh provider token: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        // Only an explicit rejected grant revokes local custody; provider outages
        // and client configuration failures must not destroy renewable authority.
        if matches!(status.as_u16(), 400 | 401)
            && response
                .json::<TokenError>()
                .await
                .is_ok_and(|body| body.error == "invalid_grant")
        {
            state.tokens = None;
            auth.store.put_subject(state.clone()).await?;
            return Ok(());
        }
        return Err(StoreError::Unavailable(
            "provider token refresh was not accepted".into(),
        ));
    }
    let body = response.json::<TokenResponse>().await.map_err(|error| {
        StoreError::Malformed(format!("provider refresh response is invalid: {error}"))
    })?;
    if body.access_token.trim().is_empty() {
        return Err(StoreError::Malformed(
            "provider refresh response contains an empty access token".into(),
        ));
    }
    if body
        .refresh_token
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
        || body.expires_in == Some(0)
    {
        return Err(StoreError::Malformed(
            "provider refresh response contains unusable token metadata".into(),
        ));
    }
    let retained_refresh = body
        .refresh_token
        .map(SecretValue::new)
        .or_else(|| state.tokens.as_ref()?.refresh_token.clone());
    let expires_at = SystemTime::now()
        .checked_add(Duration::from_secs(
            body.expires_in.unwrap_or(DEFAULT_TOKEN_LIFETIME.as_secs()),
        ))
        .ok_or_else(|| StoreError::Malformed("provider refresh lifetime is out of range".into()))?;
    state.tokens = Some(DurableSubjectTokens {
        access_token: SecretValue::new(body.access_token),
        refresh_token: retained_refresh,
        expires_at,
    });
    auth.store.put_subject(state.clone()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_margin_is_smaller_than_the_default_lifetime() {
        assert!(REFRESH_MARGIN < DEFAULT_TOKEN_LIFETIME);
    }
}
