//! Keeping delegated credentials valid underneath running sessions.
//!
//! An access token outlives neither a long terminal session nor a long agent
//! turn. Renewing it is not enough on its own: the value already handed to a
//! running runtime has to be replaced too, or the session keeps presenting a
//! credential the provider has stopped accepting.
//!
//! So renewal is a loop that does both — exchange, then push — and the runtime
//! swaps the value behind the agent without restarting it. Restarting would
//! discard the session's context, which is the one thing a terminal is for.

use std::sync::Arc;
use std::time::Duration;

use layer2_core::SessionService;

use crate::auth::{
    AuthOperationBudget, AuthPassBudget, AuthState, AUTHORIZATION_CHECK_LANE_BUDGET,
    AUTH_REFRESH_ATTEMPT_BUDGET, AUTH_REISSUE_ATTEMPT_BUDGET, AUTH_RENEWAL_PASS_BUDGET,
    AUTH_SWEEP_BATCH_SIZE, AUTH_SWEEP_TWO_LANE_BATCH_SIZE,
};

/// How often to look for credentials approaching expiry.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// How far ahead of expiry to renew.
///
/// Generous on purpose: renewing early costs one request, while renewing late
/// fails a request the user is waiting on.
const RENEW_MARGIN: Duration = Duration::from_secs(300);

/// Run the renewal loop until the process ends.
pub fn spawn(auth: Arc<AuthState>, service: Arc<SessionService>) {
    let oauth_renewal = auth.requires_credential();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            ticker.tick().await;
            let _ = sweep_once(&auth, &service).await;
        }
    });
    tracing::info!(
        sweep_seconds = SWEEP_INTERVAL.as_secs(),
        margin_seconds = RENEW_MARGIN.as_secs(),
        oauth_renewal,
        "authentication maintenance started"
    );
}

/// How long a minted session credential is kept before it is replaced.
///
/// Twelve hours is the normal rotation age for a provider key whose hard
/// expiry is thirty days. Once a credential reaches this age it remains due on
/// every bounded sweep until its replacement is acknowledged, leaving 29.5
/// days of retry margin rather than making a long session depend on one key.
const CREDENTIAL_ROTATION_AGE: Duration = Duration::from_secs(12 * 60 * 60);

/// One pass: renew what is close to expiry, then push it into live sessions.
pub(crate) async fn sweep_once(auth: &Arc<AuthState>, service: &Arc<SessionService>) -> usize {
    let pass = AuthPassBudget::new(AUTH_RENEWAL_PASS_BUDGET);
    let schema_budget = pass.lane(AUTH_SWEEP_BATCH_SIZE);
    let active_reconciliation_budget = pass.lane(AUTH_SWEEP_TWO_LANE_BATCH_SIZE);
    let provider_reconciliation_budget = pass.lane(AUTH_SWEEP_TWO_LANE_BATCH_SIZE);
    let revoke_debt_budget = pass.lane(AUTH_SWEEP_BATCH_SIZE);
    let reissue_budget = pass.lane(AUTH_SWEEP_BATCH_SIZE);
    let refresh_budget = pass.lane(AUTH_SWEEP_BATCH_SIZE);
    let authorization_page_budget = pass.lane(AUTHORIZATION_CHECK_LANE_BUDGET);

    if let Err(error) = auth.maintain_schema(&schema_budget).await {
        tracing::error!(error = %error, "could not maintain durable authentication schema");
    }

    // Durable custody whose session has ended must be retired in every mode.
    // A configuration transition can remove the issuer after a process dies,
    // but that must not leave its request-bound bearer live indefinitely.
    // Provider listing is attempted only when an issuer remains configured.
    //
    // A control plane that stopped between minting and releasing leaves one
    // behind, and nothing else records the pairing — the key's own name is
    // what says whose it was. Reclaiming needs the owner's token, which this
    // process may not hold after a restart. New keys then remain valid only to
    // their provider expiry, while legacy keys may have no bound at all. This
    // sweep is the path that takes both back promptly, so a pass it cannot make
    // is still a credential open, not a tidy-up postponed.
    match auth
        .reclaim_orphaned_credentials_with_budgets(
            service,
            &active_reconciliation_budget,
            &provider_reconciliation_budget,
        )
        .await
    {
        Ok(reclaimed) => {
            if reclaimed > 0 {
                tracing::info!(
                    count = reclaimed,
                    "reclaimed credentials no session accounts for"
                );
            }
        }
        Err(error) => {
            tracing::error!(error = %error, "could not reconcile unrecorded credentials");
        }
    }

    // Revocation and recurring session-key rotation belong to the issuer, not
    // to the interactive authentication mode. A deployment may keep its
    // platform issuer while switching from Provider to Trusted mode; durable
    // custody must still be settled and rotated after that transition.
    let has_issuer = auth.sessions_hold_own_credentials().await;
    if has_issuer {
        let taken = match auth
            .revoke_retired_credentials_with_budget(&revoke_debt_budget)
            .await
        {
            Ok(taken) => taken,
            Err(error) => {
                tracing::error!(error = %error, "could not process durable credential revoke debt");
                0
            }
        };
        if taken > 0 {
            tracing::info!(count = taken, "took back retired session credentials");
        }

        // Sessions whose own credential is old enough to swap. Distinct from
        // the subject refresh below: that keeps this service able to mint,
        // while this keeps the sessions able to act.
        let due_for_reissue = if reissue_budget.remaining() > 0 {
            match auth
                .sessions_due_for_reissue_with_budget(CREDENTIAL_ROTATION_AGE, &reissue_budget)
                .await
            {
                Ok(batch) => {
                    if let Some(error) = batch.first_error {
                        tracing::error!(
                            error = %error,
                            "one credential replacement record could not be read"
                        );
                    }
                    batch.work
                }
                Err(error) => {
                    tracing::error!(error = %error, "could not list credentials due for replacement");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        for session in due_for_reissue {
            let item_budget = AuthOperationBudget::standalone(AUTH_REISSUE_ATTEMPT_BUDGET);
            // This authoritative session read supplies both owner and grant. A
            // legacy session can require one additional environment-profile read.
            if !item_budget.take(2) {
                continue;
            }
            let (owner, services) = match service.session_credential_context(&session).await {
                Ok(Some(context)) => context,
                Ok(None) => {
                    tracing::warn!(session_id = %session, "credential replacement has no active session context");
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        session_id = %session,
                        error = %error,
                        "could not read session context for credential replacement"
                    );
                    continue;
                }
            };
            let value = match auth
                .reissue_with_budget(&session, &owner, &services, item_budget.clone())
                .await
            {
                Ok(Some(value)) => value,
                Ok(None) => {
                    tracing::warn!(session_id = %session, "could not mint a replacement credential");
                    continue;
                }
                Err(error) => {
                    tracing::error!(
                        session_id = %session,
                        error = %error,
                        "could not update durable credential replacement state"
                    );
                    continue;
                }
            };
            if !item_budget.take(1) {
                tracing::warn!(session_id = %session, "replacement remains pending because this pass exhausted its operation allowance");
                continue;
            }
            match service
                .push_session_credential_with(&session, &value, || {
                    auth.confirm_reissue_delivery_with_budget(&session, &value, &item_budget)
                })
                .await
            {
                Ok(true) => {
                    tracing::info!(session_id = %session, "session credential replaced");
                }
                Ok(false) => {
                    tracing::warn!(session_id = %session, "credential replacement did not complete delivery and durable promotion");
                }
                Err(error) => {
                    tracing::error!(
                        session_id = %session,
                        error = %error,
                        "replacement was delivered but durable promotion failed"
                    );
                }
            }
        }
    }

    // OAuth subject-token refresh is meaningful only in Provider mode. The
    // issuer work above deliberately remains active in Trusted mode.
    if !auth.requires_credential() {
        return pass.used();
    }

    let due = if refresh_budget.remaining() > 0 {
        match auth
            .subjects_needing_refresh_with_budget(RENEW_MARGIN, &refresh_budget)
            .await
        {
            Ok(batch) => {
                if let Some(error) = batch.first_error {
                    tracing::error!(
                        error = %error,
                        "one subject renewal record could not be read"
                    );
                }
                batch.work
            }
            Err(error) => {
                tracing::error!(error = %error, "could not list subjects needing credential renewal");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if !due.is_empty() {
        tracing::debug!(subjects = due.len(), "renewing delegated credentials");
    }

    /*
     * What a renewed owner token is for depends on what sessions run on.
     *
     * Where they hold keys of their own it is this service's ability to mint
     * and revoke being kept alive, and nothing else. Pushing it into sessions
     * would hand each of them the owner's whole identity — undoing, on a
     * timer, the scoping their own keys exist to provide, and silently: the
     * session keeps working, and the reach it reports stays right while the
     * credential behind it stops matching.
     *
     * Their own keys are replaced on age by the reissue above, which is the
     * rotation that belongs to a session rather than to a person.
     */
    for due in due {
        let subject = due.subject;
        let item_budget = AuthOperationBudget::standalone(AUTH_REFRESH_ATTEMPT_BUDGET);
        match auth
            .refresh_with_budget(
                &subject,
                &due.refresh_token,
                &due.incarnation,
                due.token_generation,
                &item_budget,
            )
            .await
        {
            Ok(access_token) => {
                if has_issuer {
                    // Said out loud rather than at debug. This is the branch
                    // that stops an owner's whole identity reaching their
                    // sessions, and a silent branch cannot be confirmed to
                    // have run — the only other evidence is the absence of a
                    // line, which is indistinguishable from the sweep never
                    // having fired.
                    tracing::info!(
                        %subject,
                        "renewed this owner's token; their sessions keep their own credentials"
                    );
                    continue;
                }
                if !item_budget.take(1) {
                    tracing::warn!(%subject, "renewed credential was not pushed because this pass exhausted its operation allowance");
                    continue;
                }
                let pushed = service
                    .refresh_subject_credential(&subject, access_token)
                    .await;
                tracing::info!(%subject, sessions = pushed, "credential renewed");
            }
            Err(e) => {
                // The provider has withdrawn the delegation. The session stays
                // usable as a terminal; only calls made on the owner's behalf
                // will fail, and they fail with the provider's own message
                // rather than something invented here.
                tracing::warn!(%subject, error = %e, "credential renewal failed");
            }
        }
    }
    let authorization_due_at = std::time::SystemTime::now();
    auth.reconcile_due_authorizations(authorization_due_at, &authorization_page_budget)
        .await;
    pass.used()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::persistence::{
        AuthStore, CredentialLease, DurableCredential, MemoryAuthStore, RevokeDebt, SecretValue,
        SessionCredentialState, SubjectRefreshLease,
    };
    use crate::auth::{AuthMode, SubjectTokens};
    use layer2_core::{SessionId, SubjectId};
    use std::collections::HashSet;
    use std::time::SystemTime;

    #[derive(Clone, Default)]
    struct TrustedDebtPlatform {
        deleted: Arc<std::sync::Mutex<Vec<String>>>,
    }

    async fn delete_trusted_debt(
        axum::extract::State(platform): axum::extract::State<TrustedDebtPlatform>,
        axum::extract::Path(id): axum::extract::Path<String>,
        headers: axum::http::HeaderMap,
    ) -> axum::http::StatusCode {
        assert_eq!(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        platform.deleted.lock().expect("delete log lock").push(id);
        axum::http::StatusCode::NO_CONTENT
    }

    fn tokens(expires_in: Duration, refresh: Option<&str>) -> SubjectTokens {
        SubjectTokens {
            access_token: "access".into(),
            refresh_token: refresh.map(|r| r.to_string()),
            expires_at: std::time::Instant::now() + expires_in,
        }
    }

    #[test]
    fn session_key_rotation_leaves_the_documented_provider_expiry_margin() {
        assert_eq!(
            crate::auth::keys::SESSION_KEY_LIFETIME - CREDENTIAL_ROTATION_AGE,
            Duration::from_secs((29 * 24 + 12) * 60 * 60)
        );
    }

    #[test]
    fn a_token_far_from_expiry_is_left_alone() {
        assert!(!tokens(Duration::from_secs(3600), Some("r")).needs_refresh(RENEW_MARGIN));
    }

    #[test]
    fn a_token_inside_the_margin_is_renewed_early() {
        // The margin is the point: this token is still valid, and is renewed
        // anyway so nothing fails mid-turn.
        assert!(tokens(Duration::from_secs(60), Some("r")).needs_refresh(RENEW_MARGIN));
    }

    #[test]
    fn an_already_expired_token_is_renewed() {
        assert!(tokens(Duration::from_millis(1), Some("r")).needs_refresh(RENEW_MARGIN));
    }

    /// What a renewed owner token is for, which depends on what sessions run
    /// on — and getting it wrong undoes session scoping on a timer.
    #[tokio::test]
    async fn a_renewed_owner_token_is_not_pushed_where_sessions_hold_their_own_keys() {
        /*
         * A session's own key exists so a compromised agent reaches the
         * services that session was granted and nothing else its owner can.
         * Renewing the owner's token and pushing it into every live session
         * hands each of them the whole identity back — and silently: the
         * session keeps working and the reach it reports stays right, while
         * the credential behind it stops matching.
         *
         * The renewal is still needed. It keeps this service able to mint and
         * to revoke, which is a different job from what a session presents.
         */
        let with_issuer = AuthState::new(
            AuthMode::Trusted {
                subject: "s".into(),
            },
            None,
        );
        assert!(
            !with_issuer.sessions_hold_own_credentials().await,
            "no issuer configured yet, so sessions are on their owner's token"
        );

        with_issuer
            .use_issuer(std::sync::Arc::new(crate::auth::keys::KeyIssuer::new(
                reqwest::Client::new(),
                "https://platform.example",
                "chrono-bot",
            )))
            .await;
        assert!(
            with_issuer.sessions_hold_own_credentials().await,
            "an issuer means every session has a key of its own to keep"
        );
    }

    #[tokio::test]
    async fn only_subjects_with_a_refresh_token_are_returned() {
        let auth = AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        );
        let renewable = SubjectId("has-refresh".into());
        let stranded = SubjectId("no-refresh".into());
        auth.store_tokens(&renewable, tokens(Duration::from_secs(1), Some("r")))
            .await
            .expect("renewable tokens persist");
        auth.store_tokens(&stranded, tokens(Duration::from_secs(1), None))
            .await
            .expect("stranded tokens persist");

        let due = auth
            .subjects_needing_refresh(RENEW_MARGIN)
            .await
            .expect("subject state loads");
        let subjects: Vec<_> = due
            .iter()
            .map(|refresh| refresh.subject.as_str().to_string())
            .collect();
        assert!(subjects.contains(&"has-refresh".to_string()));
        assert!(
            !subjects.contains(&"no-refresh".to_string()),
            "a subject with no refresh token cannot be renewed and must not be retried"
        );
    }

    #[tokio::test]
    async fn due_subject_pages_advance_and_wrap_without_head_of_line_starvation() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store,
        );
        for number in 0..(crate::auth::AUTH_SWEEP_BATCH_SIZE + 2) {
            auth.store_tokens(
                &SubjectId(format!("subject-{number:03}")),
                tokens(Duration::ZERO, Some("refresh")),
            )
            .await
            .expect("due subject persists");
        }

        let first = auth
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("first page loads");
        assert_eq!(first.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert_eq!(first.first().unwrap().subject.as_str(), "subject-000");
        assert_eq!(first.last().unwrap().subject.as_str(), "subject-099");

        for number in 0..crate::auth::AUTH_SWEEP_BATCH_SIZE {
            auth.store_tokens(
                &SubjectId(format!("tail-{number:03}")),
                tokens(Duration::ZERO, Some("refresh")),
            )
            .await
            .expect("growing tail persists");
        }

        let second = auth
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("second page loads");
        assert_eq!(second.len(), 2);
        assert_eq!(second.first().unwrap().subject.as_str(), "subject-100");
        assert_eq!(second.last().unwrap().subject.as_str(), "subject-101");

        let wrapped = auth
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("scan wraps");
        assert_eq!(wrapped.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert_eq!(wrapped.first().unwrap().subject.as_str(), "subject-000");
    }

    #[tokio::test]
    async fn mixed_refresh_lanes_share_capacity_and_advance_independently() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );
        let lane_work = crate::auth::AUTH_SWEEP_BATCH_SIZE / 2 + 2;
        for number in 0..lane_work {
            auth.store_tokens(
                &SubjectId(format!("unleased-{number:03}")),
                tokens(Duration::ZERO, Some("refresh")),
            )
            .await
            .expect("unleased subject persists");

            let subject = SubjectId(format!("expired-lease-{number:03}"));
            auth.store_tokens(&subject, tokens(Duration::ZERO, Some("refresh")))
                .await
                .expect("leased subject persists");
            let mut state = store
                .subject(&subject)
                .await
                .expect("store works")
                .expect("leased subject exists");
            let revision = state.revision;
            let incarnation = state.incarnation.clone();
            state.revision += 1;
            state.refresh_lease = Some(SubjectRefreshLease {
                holder: "abandoned-worker".into(),
                expires_at: SystemTime::now() - Duration::from_secs(1),
            });
            assert!(store
                .replace_subject(&incarnation, revision, state)
                .await
                .expect("expired lease persists"));
        }

        let first = auth
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("first mixed page loads");
        assert_eq!(first.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert_eq!(
            first
                .iter()
                .filter(|work| work.subject.as_str().starts_with("unleased-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 2
        );
        assert_eq!(
            first
                .iter()
                .filter(|work| work.subject.as_str().starts_with("expired-lease-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 2
        );

        let second = auth
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("second mixed page loads");
        assert_eq!(second.len(), 4);
        assert_eq!(
            second
                .iter()
                .filter(|work| work.subject.as_str().starts_with("unleased-"))
                .count(),
            2
        );
        assert_eq!(
            second
                .iter()
                .filter(|work| work.subject.as_str().starts_with("expired-lease-"))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn a_failed_later_refresh_lane_does_not_publish_the_earlier_epoch() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );
        for number in 0..60 {
            auth.store_tokens(
                &SubjectId(format!("unleased-{number:03}")),
                tokens(Duration::ZERO, Some("refresh")),
            )
            .await
            .expect("unleased subject persists");
        }
        let leased = SubjectId("expired-lease".into());
        auth.store_tokens(&leased, tokens(Duration::ZERO, Some("refresh")))
            .await
            .expect("leased subject persists");
        let mut state = store
            .subject(&leased)
            .await
            .unwrap()
            .expect("leased subject exists");
        let revision = state.revision;
        let incarnation = state.incarnation.clone();
        state.revision += 1;
        state.refresh_lease = Some(SubjectRefreshLease {
            holder: "abandoned-worker".into(),
            expires_at: SystemTime::now() - Duration::from_secs(1),
        });
        assert!(store
            .replace_subject(&incarnation, revision, state)
            .await
            .unwrap());

        store.fail_scan_on_call(4);
        assert!(matches!(
            auth.subjects_needing_refresh(Duration::ZERO).await,
            Err(layer2_core::store::StoreError::Unavailable(_))
        ));
        store.clear_scan_failure();

        let retry = auth
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("retry starts from the unpublished epoch");
        assert!(retry
            .iter()
            .any(|work| work.subject.as_str() == "unleased-000"));
    }

    #[tokio::test]
    async fn mixed_reissue_lanes_share_capacity_and_advance_independently() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store.clone(),
        );
        let owner = SubjectId("owner".into());
        let issued_at = SystemTime::now() - Duration::from_secs(60);
        let credential = |id: String| DurableCredential {
            id,
            value: SecretValue::new("credential-value".into()),
            owner: owner.clone(),
            issued_at,
            expires_at: None,
            scope_fingerprint: "scope".into(),
            revocation_credential: None,
        };
        let ordinary_lane_work = crate::auth::AUTH_SWEEP_BATCH_SIZE / 3 + 2;
        for number in 0..ordinary_lane_work {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("current-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(format!("provider-current-{number:03}"))),
                    pending: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("current state persists"));
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("pending-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(format!("provider-old-{number:03}"))),
                    pending: Some(credential(format!("provider-pending-{number:03}"))),
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("pending state persists"));
        }
        let lease_lane_size =
            crate::auth::AUTH_SWEEP_BATCH_SIZE - 2 * (crate::auth::AUTH_SWEEP_BATCH_SIZE / 3);
        for number in 0..(lease_lane_size + 2) {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("expired-lease-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(format!("provider-leased-{number:03}"))),
                    pending: None,
                    revoke_debt: Vec::new(),
                    lease: Some(CredentialLease {
                        holder: "abandoned-worker".into(),
                        fence: 1,
                        acquired_at: issued_at,
                        expires_at: SystemTime::now() - Duration::from_secs(1),
                        scope_fingerprint: "scope".into(),
                    }),
                    retired: false,
                })
                .await
                .expect("expired lease state persists"));
        }

        let first = auth
            .sessions_due_for_reissue(Duration::ZERO)
            .await
            .expect("first mixed page loads");
        assert_eq!(first.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("current-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 3
        );
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("pending-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 3
        );
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("expired-lease-"))
                .count(),
            lease_lane_size
        );

        for number in 0..crate::auth::AUTH_SWEEP_BATCH_SIZE {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("pending-tail-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(format!("provider-tail-old-{number:03}"))),
                    pending: Some(credential(format!("provider-tail-pending-{number:03}"))),
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("growing pending tail persists"));
        }

        let second = auth
            .sessions_due_for_reissue(Duration::ZERO)
            .await
            .expect("second mixed page loads");
        assert_eq!(second.len(), 6);
        assert_eq!(
            second
                .iter()
                .filter(|session| session.as_str().starts_with("current-"))
                .count(),
            2
        );

        let wrapped = auth
            .sessions_due_for_reissue(Duration::ZERO)
            .await
            .expect("finite epochs wrap despite a sustained tail");
        assert_eq!(wrapped.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert!(wrapped
            .iter()
            .any(|session| session.as_str() == "pending-000"));
        assert_eq!(
            second
                .iter()
                .filter(|session| session.as_str().starts_with("expired-lease-"))
                .count(),
            2
        );
        assert_eq!(
            second
                .iter()
                .filter(|session| session.as_str().starts_with("pending-"))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn a_failed_later_reissue_lane_does_not_publish_the_earlier_epoch() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store.clone(),
        );
        let owner = SubjectId("owner".into());
        let issued_at = SystemTime::now() - Duration::from_secs(60);
        let credential = |id: String| DurableCredential {
            id,
            value: SecretValue::new("credential-value".into()),
            owner: owner.clone(),
            issued_at,
            expires_at: None,
            scope_fingerprint: "scope".into(),
            revocation_credential: None,
        };
        for number in 0..40 {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("current-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(format!("provider-current-{number:03}"))),
                    pending: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .unwrap());
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("pending-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(format!("provider-old-{number:03}"))),
                    pending: Some(credential(format!("provider-pending-{number:03}"))),
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .unwrap());
        }

        store.fail_scan_on_call(5);
        assert!(matches!(
            auth.sessions_due_for_reissue(Duration::ZERO).await,
            Err(layer2_core::store::StoreError::Unavailable(_))
        ));
        store.clear_scan_failure();

        let retry = auth
            .sessions_due_for_reissue(Duration::ZERO)
            .await
            .expect("retry starts from the unpublished epochs");
        assert!(retry
            .iter()
            .any(|session| session.as_str() == "pending-000"));
        assert!(retry
            .iter()
            .any(|session| session.as_str() == "current-000"));
    }

    #[tokio::test]
    async fn a_stored_access_token_is_readable_for_delegation() {
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
        );
        let s = SubjectId("subject-a".into());
        assert_eq!(auth.access_token(&s).await.expect("state loads"), None);
        auth.store_tokens(&s, tokens(Duration::from_secs(600), Some("r")))
            .await
            .expect("tokens persist");
        assert_eq!(
            auth.access_token(&s).await.expect("state loads").as_deref(),
            Some("access")
        );
    }

    #[tokio::test]
    async fn shared_pass_budget_caps_saturated_lanes_while_fair_cursors_progress() {
        const REISSUE_PER_SUBLANE: usize = 6;
        const REFRESH_PER_SUBLANE: usize = 12;

        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
            store.clone(),
        );
        let owner = SubjectId("owner".into());
        let issued_at = SystemTime::now() - Duration::from_secs(60);
        let credential = |id: String| DurableCredential {
            id,
            value: SecretValue::new("value".into()),
            owner: owner.clone(),
            issued_at,
            expires_at: None,
            scope_fingerprint: "scope".into(),
            revocation_credential: None,
        };
        for number in 0..REISSUE_PER_SUBLANE {
            for (kind, pending, lease) in [
                ("current", false, false),
                ("pending", true, false),
                ("lease", false, true),
            ] {
                let session = SessionId(format!("{kind}-{number:03}"));
                assert!(store
                    .insert_credential(SessionCredentialState {
                        session,
                        generation: 0,
                        request_bound_credential: None,
                        current: Some(credential(format!("provider-{kind}-{number:03}"))),
                        pending: pending
                            .then(|| credential(format!("provider-pending-{number:03}"))),
                        revoke_debt: Vec::new(),
                        lease: lease.then(|| CredentialLease {
                            holder: "expired-worker".into(),
                            fence: 1,
                            acquired_at: issued_at,
                            expires_at: SystemTime::now() - Duration::from_secs(1),
                            scope_fingerprint: "scope".into(),
                        }),
                        retired: false,
                    })
                    .await
                    .expect("credential work persists"));
            }
        }
        for number in 0..REFRESH_PER_SUBLANE {
            auth.store_tokens(
                &SubjectId(format!("unleased-{number:03}")),
                tokens(Duration::ZERO, Some("refresh")),
            )
            .await
            .expect("unleased refresh work persists");
            let subject = SubjectId(format!("leased-{number:03}"));
            auth.store_tokens(&subject, tokens(Duration::ZERO, Some("refresh")))
                .await
                .expect("leased refresh work persists");
            let mut state = store.subject(&subject).await.unwrap().unwrap();
            let revision = state.revision;
            let incarnation = state.incarnation.clone();
            state.revision += 1;
            state.refresh_lease = Some(SubjectRefreshLease {
                holder: "expired-worker".into(),
                expires_at: SystemTime::now() - Duration::from_secs(1),
            });
            assert!(store
                .replace_subject(&incarnation, revision, state)
                .await
                .unwrap());
        }

        let mut reissued = HashSet::new();
        let mut refreshed = HashSet::new();
        for _ in 0..3 {
            let pass = AuthPassBudget::new(AUTH_RENEWAL_PASS_BUDGET);
            let schema = pass.lane(AUTH_SWEEP_BATCH_SIZE);
            let active = pass.lane(AUTH_SWEEP_TWO_LANE_BATCH_SIZE);
            let provider = pass.lane(AUTH_SWEEP_TWO_LANE_BATCH_SIZE);
            let debt = pass.lane(AUTH_SWEEP_BATCH_SIZE);
            let reissue = pass.lane(AUTH_SWEEP_BATCH_SIZE);
            let refresh = pass.lane(AUTH_SWEEP_BATCH_SIZE);
            let authorization = pass.lane(AUTHORIZATION_CHECK_LANE_BUDGET);
            assert!(schema.take(AUTH_SWEEP_BATCH_SIZE));
            assert!(active.take(AUTH_SWEEP_TWO_LANE_BATCH_SIZE));
            assert!(provider.take(AUTH_SWEEP_TWO_LANE_BATCH_SIZE));
            assert!(debt.take(AUTH_SWEEP_BATCH_SIZE));

            let due_reissue = auth
                .sessions_due_for_reissue_with_budget(Duration::ZERO, &reissue)
                .await
                .expect("bounded reissue selection succeeds");
            for session in due_reissue {
                assert!(
                    reissued.insert(session.0),
                    "cursor must not repeat before wrap"
                );
            }
            let due_refresh = auth
                .subjects_needing_refresh_with_budget(Duration::ZERO, &refresh)
                .await
                .expect("bounded refresh selection succeeds");
            for due in due_refresh {
                assert!(
                    refreshed.insert(due.subject.0),
                    "cursor must not repeat before wrap"
                );
            }
            assert!(pass.used() <= AUTH_RENEWAL_PASS_BUDGET);

            let reissue_remaining = reissue.remaining();
            let refresh_remaining = refresh.remaining();
            let authorization_remaining = authorization.remaining();
            assert!(reissue.take(reissue_remaining));
            assert!(refresh.take(refresh_remaining));
            assert!(authorization.take(authorization_remaining));
            assert_eq!(
                pass.used(),
                AUTH_RENEWAL_PASS_BUDGET,
                "even fully saturated lanes share one hard pass ceiling"
            );
        }

        assert_eq!(reissued.len(), REISSUE_PER_SUBLANE * 3);
        assert_eq!(refreshed.len(), REFRESH_PER_SUBLANE * 2);
        for prefix in ["current-", "pending-", "lease-"] {
            assert_eq!(
                reissued
                    .iter()
                    .filter(|session| session.starts_with(prefix))
                    .count(),
                REISSUE_PER_SUBLANE
            );
        }
        for prefix in ["unleased-", "leased-"] {
            assert_eq!(
                refreshed
                    .iter()
                    .filter(|subject| subject.starts_with(prefix))
                    .count(),
                REFRESH_PER_SUBLANE
            );
        }
    }

    #[tokio::test]
    async fn trusted_mode_with_an_issuer_processes_durable_revoke_debt() {
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store.clone(),
        );
        assert!(!auth.requires_credential());

        let owner = SubjectId("owner".into());
        let session = SessionId("sesn_trusted_mode_debt".into());
        store
            .insert_credential(SessionCredentialState {
                session: session.clone(),
                generation: 0,
                request_bound_credential: None,
                current: None,
                pending: None,
                revoke_debt: vec![RevokeDebt {
                    id: "provider-key".into(),
                    owner,
                    queued_at: SystemTime::now(),
                    revocation_credential: Some(SecretValue::new("owner-access".into())),
                }],
                lease: None,
                retired: true,
            })
            .await
            .expect("revoke debt persists");

        let platform = TrustedDebtPlatform::default();
        let app = axum::Router::new()
            .route(
                "/api/v1/api-keys/{id}",
                axum::routing::delete(delete_trusted_debt),
            )
            .with_state(platform.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind revoke platform");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve revoke platform");
        });
        auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;

        let service = crate::auth::empty_session_service();
        sweep_once(&auth, &service).await;

        assert_eq!(
            *platform.deleted.lock().expect("delete log lock"),
            vec!["provider-key".to_string()]
        );
        assert!(store
            .credential(&session)
            .await
            .expect("store works")
            .expect("settled terminal custody remains until TTL cleanup")
            .revoke_debt
            .is_empty());
        server.abort();
    }
}
