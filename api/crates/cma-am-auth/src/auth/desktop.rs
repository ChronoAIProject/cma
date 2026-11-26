//! Exact browser/bearer proof for long-lived replica-mode desktop streams.
use super::{
    cookie_authorization, identity,
    persistence::{SecretDigest, SecretValue, SubjectAuthState},
    session_routes, AuthMode, AuthState, RequestAuth, AUTH_COOKIE, AUTH_COOKIE_DOMAIN,
};
use axum::http::HeaderMap;
use layer2_core::resource_access::CredentialSource;
use layer2_core::{
    sandbox::{DesktopAuthorization, DesktopError},
    Caller, Capability, SubjectId,
};
use std::{sync::Arc, time::SystemTime};

struct Authority {
    auth: Arc<AuthState>,
    subject: SubjectId,
    proof: Proof,
}

enum Proof {
    Browser {
        login: SecretDigest,
        token: SecretValue,
        state: Box<tokio::sync::OnceCell<SubjectAuthState>>,
    },
    Bearer {
        token: SecretValue,
        source: CredentialSource,
    },
    IdentityProxy {
        token: SecretValue,
        source: CredentialSource,
    },
    Trusted,
}

/// Called only by trusted middleware after its ordinary request verification.
/// Capture no authority from a caller-provided peer flag or fallback cache.
pub(super) fn capture(
    auth: Arc<AuthState>,
    headers: &HeaderMap,
    resolved: &RequestAuth,
) -> Result<Arc<dyn DesktopAuthorization>, DesktopError> {
    if resolved.binding.is_some() || !resolved.authorization.allows(Capability::UseOwnSandboxes) {
        return Err(DesktopError::Unavailable);
    }
    let proof = match &auth.mode {
        AuthMode::Trusted { subject } if subject == resolved.subject.as_str() => Proof::Trusted,
        _ if resolved.credential_source == CredentialSource::Retained => {
            let cookie = session_routes::request_cookie(headers, AUTH_COOKIE)
                .ok_or(DesktopError::Unavailable)?;
            let login = SecretDigest::identify(AUTH_COOKIE_DOMAIN, cookie);
            let token = resolved
                .delegated_credential
                .as_ref()
                .ok_or(DesktopError::Unavailable)?;
            Proof::Browser {
                login,
                token: SecretValue::new(token.clone()),
                state: Box::default(),
            }
        }
        _ if matches!(
            resolved.credential_source,
            CredentialSource::Independent { .. }
        ) =>
        {
            let token = resolved
                .delegated_credential
                .as_ref()
                .ok_or(DesktopError::Unavailable)?;
            Proof::Bearer {
                token: SecretValue::new(token.clone()),
                source: resolved.credential_source,
            }
        }
        _ if matches!(
            resolved.credential_source,
            CredentialSource::IdentityProxy { .. }
        ) =>
        {
            let token = headers
                .get(super::nyxid_proxy::HEADER)
                .and_then(|value| value.to_str().ok())
                .ok_or(DesktopError::Unavailable)?;
            Proof::IdentityProxy {
                token: SecretValue::new(token.to_owned()),
                source: resolved.credential_source,
            }
        }
        // Request-bound/delegated read proofs require their own reviewed
        // input-authority semantics; they cannot silently become owner custody.
        _ => return Err(DesktopError::Unavailable),
    };
    Ok(Arc::new(Authority {
        auth,
        subject: resolved.subject.clone(),
        proof,
    }))
}

#[async_trait::async_trait]
impl DesktopAuthorization for Authority {
    async fn current(&self, caller: &Caller) -> Result<(), DesktopError> {
        if caller.subject != self.subject {
            return Err(DesktopError::Unavailable);
        }
        let resolved = match &self.proof {
            Proof::Trusted => {
                return match &self.auth.mode {
                    AuthMode::Trusted { subject }
                        if subject == self.subject.as_str()
                            && caller.authorization.allows(Capability::UseOwnSandboxes) =>
                    {
                        Ok(())
                    }
                    _ => Err(DesktopError::Unavailable),
                };
            }
            Proof::Browser {
                login,
                token,
                state,
            } => {
                require_login(&self.auth, login, &self.subject).await?;
                // Capture lazily: switch-off connections incur no additional
                // store/provider work. Once selected, never adopt another login
                // incarnation or a replacement credential for this socket.
                let state =
                    state
                        .get_or_try_init(|| async {
                            let state = self
                                .auth
                                .store
                                .subject(&self.subject)
                                .await
                                .map_err(|_| DesktopError::AuthorityUnavailable)?
                                .ok_or(DesktopError::Unavailable)?;
                            if !state.tokens.as_ref().is_some_and(|tokens| {
                                tokens.access_token.expose() == token.expose()
                            }) {
                                return Err(DesktopError::Unavailable);
                            }
                            Ok(state)
                        })
                        .await?;
                let resolved = cookie_authorization::request_for_browser_cookie(
                    &self.auth,
                    &self.subject,
                    state.clone(),
                )
                .await
                .map_err(|_| DesktopError::AuthorityUnavailable)?
                .ok_or(DesktopError::Unavailable)?;
                // Provider I/O can overlap logout on another manager.
                require_login(&self.auth, login, &self.subject).await?;
                resolved
            }
            Proof::IdentityProxy { token, source } => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    super::nyxid_proxy::HEADER,
                    token
                        .expose()
                        .parse()
                        .map_err(|_| DesktopError::Unavailable)?,
                );
                let resolved = self
                    .auth
                    .resolve_request(&headers)
                    .await
                    .map_err(|_| DesktopError::AuthorityUnavailable)?
                    .ok_or(DesktopError::Unavailable)?;
                if resolved.credential_source != *source {
                    return Err(DesktopError::Unavailable);
                }
                resolved
            }
            Proof::Bearer { token, source } => {
                let resolved = identity::request_for_bearer(&self.auth, token.expose().to_owned())
                    .await
                    .map_err(|_| DesktopError::AuthorityUnavailable)?
                    .ok_or(DesktopError::Unavailable)?;
                if resolved.credential_source != *source {
                    return Err(DesktopError::Unavailable);
                }
                resolved
            }
        };
        if resolved.subject != self.subject
            || resolved.binding.is_some()
            || !resolved.authorization.allows(Capability::UseOwnSandboxes)
        {
            return Err(DesktopError::Unavailable);
        }
        Ok(())
    }
}

async fn require_login(
    auth: &AuthState,
    login: &SecretDigest,
    subject: &SubjectId,
) -> Result<(), DesktopError> {
    let current = auth
        .store
        .browser_login(login)
        .await
        .map_err(|_| DesktopError::AuthorityUnavailable)?
        .ok_or(DesktopError::Unavailable)?;
    if &current.subject != subject || current.expires_at <= SystemTime::now() {
        return Err(DesktopError::Unavailable);
    }
    Ok(())
}
