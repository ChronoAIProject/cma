//! Fresh exact-caller proof before public owner lookup. No completed-result cache.
use super::{cookie_authorization, persistence::*, session_routes, AuthMode, AuthState};
use axum::http::{HeaderMap, StatusCode};
use layer2_core::resource_access::{CallerContext, CredentialSource};
use std::{sync::Arc, time::SystemTime};

pub struct RelayCaller {
    auth: Arc<AuthState>,
    context: CallerContext,
    browser: Option<(SecretDigest, SubjectAuthState)>,
}

impl RelayCaller {
    pub async fn verify(
        auth: Arc<AuthState>,
        headers: &HeaderMap,
        context: &CallerContext,
        credential: Option<&str>,
    ) -> Result<Self, StatusCode> {
        valid(context)?;
        if matches!(
            context.credential_source,
            CredentialSource::RequestBoundRead { .. }
        ) {
            // Ordinary middleware already verified the exact method/path/query proof.
            // Its deadline remains attached; ambient credentials never replace it.
            return Ok(Self {
                auth,
                context: context.clone(),
                browser: None,
            });
        }
        let browser = if matches!(auth.mode, AuthMode::Provider { .. })
            && context.credential_source == CredentialSource::Retained
        {
            let cookie = session_routes::request_cookie(headers, super::AUTH_COOKIE)
                .ok_or(StatusCode::UNAUTHORIZED)?;
            let login = SecretDigest::identify(super::AUTH_COOKIE_DOMAIN, cookie);
            require_login(&auth, &login, context).await?;
            let state = auth
                .store
                .subject(&context.caller.subject)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                .ok_or(StatusCode::UNAUTHORIZED)?;
            if !state.tokens.as_ref().is_some_and(|tokens| {
                Some(tokens.access_token.expose()) == credential
                    && tokens.expires_at > SystemTime::now()
            }) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            Some((login, state))
        } else {
            None
        };
        let resolved = match &browser {
            Some((_, state)) => {
                cookie_authorization::request_for_browser_cookie(
                    &auth,
                    &context.caller.subject,
                    state.clone(),
                )
                .await
            }
            None => auth.resolve_request(headers).await,
        }
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
        if resolved.subject != context.caller.subject
            || resolved.authorization != context.caller.authorization
            || resolved.credential_source != context.credential_source
            || resolved.binding.is_some()
            || resolved.delegated_credential.as_deref() != credential
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let proof = Self {
            auth,
            context: context.clone(),
            browser,
        };
        proof.current().await?;
        Ok(proof)
    }

    /// Policy IO may overlap logout, credential replacement or expiry.
    pub async fn current(&self) -> Result<(), StatusCode> {
        valid(&self.context)?;
        if let Some((login, state)) = &self.browser {
            cookie_authorization::current_custody(&self.auth, state)
                .await
                .map_err(|error| match error {
                    cookie_authorization::CustodyError::Changed => StatusCode::UNAUTHORIZED,
                    cookie_authorization::CustodyError::Store(_) => StatusCode::SERVICE_UNAVAILABLE,
                })?;
            require_login(&self.auth, login, &self.context).await?;
        }
        valid(&self.context)
    }
}

fn valid(context: &CallerContext) -> Result<(), StatusCode> {
    if !context.validate_principal()
        || context
            .credential_deadline()
            .is_some_and(|deadline| deadline <= tokio::time::Instant::now())
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

async fn require_login(
    auth: &AuthState,
    login: &SecretDigest,
    context: &CallerContext,
) -> Result<(), StatusCode> {
    if !auth
        .store
        .browser_login(login)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .is_some_and(|current| {
            current.subject == context.caller.subject && current.expires_at > SystemTime::now()
        })
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod tests;
