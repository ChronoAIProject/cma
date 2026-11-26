//! Complete one browser-bound attempt without recreating revoked custody.
use super::*;

pub(super) async fn callback(
    State(auth): State<Arc<AuthState>>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    callback_for_scope(auth, params, headers, true).await
}

pub(super) async fn account_callback(
    State(auth): State<Arc<AuthState>>,
    Query(params): Query<CallbackParams>,
    headers: HeaderMap,
) -> Response {
    callback_for_scope(auth, params, headers, false).await
}

async fn callback_for_scope(
    auth: Arc<AuthState>,
    params: CallbackParams,
    headers: HeaderMap,
    actor_links: bool,
) -> Response {
    if !matches!(auth.mode, AuthMode::Provider { .. }) {
        return Redirect::to("/").into_response();
    }
    let Some(state) = params.state.as_deref() else {
        return login_failure(StatusCode::BAD_REQUEST, "login_state_missing", None);
    };
    let cookie_name = binding_cookie_name(state);
    let Some(binding) = request_cookie(&headers, &cookie_name) else {
        return silent::fallback(
            state.starts_with("s."),
            None,
            login_failure(
                StatusCode::BAD_REQUEST,
                "login_browser_binding_missing",
                None,
            ),
        );
    };
    let attempt = match auth
        .store
        .take_login_attempt(
            &SecretDigest::identify(ATTEMPT_DOMAIN, state),
            &SecretDigest::identify(BINDING_DOMAIN, binding),
        )
        .await
    {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            return silent::fallback(
                state.starts_with("s."),
                None,
                login_failure(StatusCode::BAD_REQUEST, "login_state_invalid", None),
            )
        }
        Err(error) => {
            tracing::error!(error = %error, "could not consume login attempt");
            return silent::fallback(state.starts_with("s."), None, store_failure());
        }
    };
    let mut response = if !actor_links && attempt.actor_link_intent.is_some() {
        layer1_public::rest::HttpProblem::SandboxBackendUnavailable.into_response()
    } else {
        complete_callback(&auth, &params, &headers, &attempt, binding).await
    };
    if !response.headers().contains_key(header::SET_COOKIE) {
        if let Err(error) = auth.revoke_browser_binding(binding).await {
            tracing::error!(error = %error, "could not cancel browser login");
            return silent::fallback(
                attempt.silent,
                attempt.return_to.as_deref(),
                store_failure(),
            );
        }
    }
    if attempt.silent && !response.status().is_redirection() {
        response = Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response();
    }
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(&cookie_name, "", auth.secure_cookies(), 0),
    );
    response
}

async fn complete_callback(
    auth: &Arc<AuthState>,
    params: &CallbackParams,
    headers: &HeaderMap,
    attempt: &LoginAttempt,
    binding: &str,
) -> Response {
    let AuthMode::Provider {
        client_id,
        client_secret,
        public_url,
        ..
    } = &auth.mode
    else {
        unreachable!("provider callback checked before consuming state")
    };
    if attempt.expires_at <= SystemTime::now() {
        return login_failure(StatusCode::BAD_REQUEST, "login_state_expired", None);
    }
    if !attempt.custody_reserved {
        // Predecessors did not reserve custody. Adopt the valid, one-use attempt
        // before provider IO, but never overwrite a logout tombstone.
        match auth
            .store
            .reserve_browser_login(
                SecretDigest::identify(AUTH_COOKIE_DOMAIN, binding),
                attempt.expires_at,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => return login_failure(StatusCode::UNAUTHORIZED, "login_superseded", None),
            Err(_) => return store_failure(),
        }
    }
    if attempt.silent {
        if let Some(response) =
            silent::declined_callback(auth, attempt, params.error.as_deref(), headers, binding)
                .await
        {
            return response;
        }
    }
    if params.error.is_some() {
        return login_failure(StatusCode::UNAUTHORIZED, "login_cancelled", None);
    }
    let Some(code) = params
        .code
        .as_deref()
        .filter(|code| !code.trim().is_empty())
    else {
        return login_failure(StatusCode::BAD_REQUEST, "login_code_missing", None);
    };
    let metadata = match auth.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => return provider_failure("identity_discovery_failed", error),
    };
    let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
    let response = match auth
        .http
        .post(&metadata.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code_verifier", attempt.verifier.expose()),
        ])
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => return provider_failure("identity_token_exchange_failed", error.to_string()),
    };
    if !response.status().is_success() {
        return provider_failure(
            "identity_token_exchange_failed",
            format!(
                "identity token endpoint returned HTTP {}",
                response.status().as_u16()
            ),
        );
    }
    let token = match response.json::<TokenResponse>().await {
        Ok(token) if !token.access_token.trim().is_empty() => token,
        _ => return provider_failure("identity_token_exchange_failed", "invalid token response"),
    };
    let lifetime = Duration::from_secs(
        token
            .expires_in
            .unwrap_or(DEFAULT_ACCESS_TOKEN_LIFETIME.as_secs()),
    )
    .min(MAX_ACCESS_TOKEN_LIFETIME);
    let (subject, permission_names, profile) =
        match identity::resolve_subject(auth, &metadata, &token.access_token).await {
            Ok(identity) => identity,
            Err(error) => return provider_failure("identity_resolution_failed", error),
        };
    // Registration is decided before the subject is persisted or a browser
    // login is minted (platform spend decision 25). The caller clears the
    // binding cookie and, as no cookie is set here, cancels the browser login.
    match auth.admit_registration(&subject).await {
        Ok(super::super::records::RegistrationDecision::Closed) => {
            return super::super::records::registration_closed(None)
        }
        Ok(_) => {}
        Err(error) => {
            tracing::error!(error = %error, "could not read the platform admission state");
            return store_failure();
        }
    }
    let authorization = crate::permissions::capabilities_from_permissions(permission_names);
    match auth.standing.authoritative(&subject).await {
        Ok(false) => {}
        Ok(true) => return crate::auth::account_standing::deletion_problem(),
        Err(_) => return store_failure(),
    }
    if let Err(error) = auth
        .store_sign_in(
            subject.clone(),
            DurableSubjectTokens {
                access_token: SecretValue::new(token.access_token),
                refresh_token: token.refresh_token.map(SecretValue::new),
                expires_at: SystemTime::now() + lifetime,
            },
            authorization,
            profile.clone(),
        )
        .await
    {
        tracing::error!(error = %error, "could not persist signed-in subject");
        return store_failure();
    }
    auth.note_sign_in(&subject, &profile).await;
    let explicit_started_at = (!attempt.silent).then(|| {
        attempt.started_at.unwrap_or_else(|| {
            attempt
                .expires_at
                .checked_sub(ATTEMPT_LIFETIME)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
    });
    // Never turn a pre-authentication secret into authenticated authority.
    let browser_login = random_token();
    match auth
        .store
        .complete_browser_login(
            &SecretDigest::identify(AUTH_COOKIE_DOMAIN, binding),
            BrowserLogin {
                id: SecretDigest::identify(AUTH_COOKIE_DOMAIN, &browser_login),
                subject: subject.clone(),
                expires_at: SystemTime::now() + AUTH_COOKIE_LIFETIME,
            },
            explicit_started_at,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => return login_failure(StatusCode::UNAUTHORIZED, "login_superseded", None),
        Err(error) => {
            tracing::error!(error=%error,"could not complete browser login");
            return store_failure();
        }
    }
    if auth
        .retain_actor_login_completion(attempt, &browser_login, &subject)
        .await
        .is_err()
    {
        return login_failure(StatusCode::FORBIDDEN, "actor_login_intent_expired", None);
    }
    let mut response = Redirect::to(attempt.return_to.as_deref().unwrap_or("/")).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        issued_cookie(
            AUTH_COOKIE,
            &browser_login,
            auth.secure_cookies(),
            AUTH_COOKIE_LIFETIME.as_secs(),
        ),
    );
    if let Some(started) = explicit_started_at {
        crate::auth::silent_preference::record_sign_in(
            &mut response,
            &SecretDigest::identify(AUTH_COOKIE_DOMAIN, binding),
            started,
            auth.secure_cookies(),
        );
    }
    response
}
