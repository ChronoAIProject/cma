//! Request-local cookie authority, fenced against concurrent token replacement.

use std::time::{Duration, SystemTime};

use layer2_core::{Authorization, StoreError, SubjectId};

use super::{
    identity, permissions::PermissionEvidence, persistence::SubjectAuthState, AuthState,
    RequestAuth,
};

const PROVIDER_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) async fn request_for_cookie(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
) -> Result<RequestAuth, StoreError> {
    request_with_timeout(auth, owner, state, PROVIDER_VERIFICATION_TIMEOUT).await
}

async fn request_with_timeout(
    auth: &AuthState,
    owner: &SubjectId,
    state: SubjectAuthState,
    timeout: Duration,
) -> Result<RequestAuth, StoreError> {
    if &state.subject != owner {
        return Err(unavailable());
    }
    let Some(tokens) = state.tokens.as_ref().filter(|tokens| {
        tokens.expires_at > SystemTime::now() && !tokens.access_token.expose().trim().is_empty()
    }) else {
        return Ok(RequestAuth {
            subject: state.subject,
            authorization: Authorization::none(),
            profile: state.profile,
            delegated_credential: None,
        });
    };
    let verified = tokio::time::timeout(timeout, async {
        let metadata = auth.metadata().await?;
        identity::verify_subject(auth, &metadata, tokens.access_token.expose()).await
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?;
    if &verified.subject != owner {
        return Err(unavailable());
    }

    // Provider evidence describes exactly this token generation, not a later login.
    let current = auth.store.subject(owner).await?.ok_or_else(unavailable)?;
    let unchanged = current.subject == state.subject
        && current.incarnation == state.incarnation
        && current.token_generation == state.token_generation
        && current.tokens.as_ref().is_some_and(|current_tokens| {
            current_tokens.access_token == tokens.access_token
                && current_tokens.expires_at > SystemTime::now()
        });
    if !unchanged {
        return Err(unavailable());
    }
    let (authorization, delegated_credential) = match verified.permissions {
        PermissionEvidence::Active(permissions) => (
            crate::permissions::capabilities_from_permissions(permissions),
            Some(tokens.access_token.expose().to_owned()),
        ),
        PermissionEvidence::Inactive => (Authorization::none(), None),
    };
    Ok(RequestAuth {
        subject: verified.subject,
        authorization,
        profile: verified.profile,
        delegated_credential,
    })
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("cookie authorization could not be verified".into())
}

#[cfg(test)]
#[path = "cookie_authorization_tests.rs"]
mod tests;
