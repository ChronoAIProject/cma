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

#[path = "refresh_lease.rs"]
mod lease;

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

pub async fn refresh_if_needed(
    auth: &AuthState,
    state: &mut SubjectAuthState,
) -> Result<(), StoreError> {
    if !due(state) || !auth.requires_credential() {
        return Ok(());
    }
    if let Some(tokens) = &state.tokens {
        auth.identity_store
            .invalidate_token(tokens.access_token.expose());
    }
    let (claim, observed) = match lease::acquire(auth, state).await? {
        lease::Acquisition::Current(current) => {
            *state = current;
            return Ok(());
        }
        lease::Acquisition::Owned(claim, observed) => (claim, observed),
    };
    let result = exchange(auth, &observed).await;
    match result {
        Ok(tokens) => {
            if let Some(current) = claim.finish(auth, tokens).await? {
                *state = current;
                return Ok(());
            }
            let current = lease::read(auth, &state.subject).await?;
            let changed = lease::changed(&observed, &current);
            *state = current;
            if changed {
                Ok(())
            } else {
                Err(StoreError::Unavailable(
                    "subject refresh lease expired or was lost".into(),
                ))
            }
        }
        Err(error) => {
            claim.abandon(auth).await?;
            let current = lease::read(auth, &state.subject).await?;
            let changed = lease::changed(&observed, &current);
            *state = current;
            if changed {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

fn due(state: &SubjectAuthState) -> bool {
    let Some(tokens) = state.tokens.as_ref() else {
        return false;
    };
    let renewable = tokens
        .refresh_token
        .as_ref()
        .is_some_and(|token| !token.expose().trim().is_empty());
    let refresh_at = if renewable {
        tokens
            .expires_at
            .checked_sub(REFRESH_MARGIN)
            .unwrap_or(SystemTime::UNIX_EPOCH)
    } else {
        tokens.expires_at
    };
    SystemTime::now() >= refresh_at
}

async fn exchange(
    auth: &AuthState,
    state: &SubjectAuthState,
) -> Result<Option<DurableSubjectTokens>, StoreError> {
    let Some(refresh_token) = state
        .tokens
        .as_ref()
        .and_then(|tokens| tokens.refresh_token.as_ref())
        .filter(|token| !token.expose().trim().is_empty())
    else {
        return Ok(None);
    };
    let AuthMode::Provider {
        client_id,
        client_secret,
        ..
    } = &auth.mode
    else {
        return Err(StoreError::Unavailable(
            "no refresh provider configured".into(),
        ));
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
        if status.as_u16() == 400
            && response
                .json::<TokenError>()
                .await
                .is_ok_and(|body| body.error == "invalid_grant")
        {
            if let Some(tokens) = &state.tokens {
                auth.identity_store
                    .invalidate_token(tokens.access_token.expose());
            }
            return Ok(None);
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
    Ok(Some(DurableSubjectTokens {
        access_token: SecretValue::new(body.access_token),
        refresh_token: retained_refresh,
        expires_at,
    }))
}
