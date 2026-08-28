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
    credential_replace_before, AuthOperationBudget, AuthPassBudget, AuthState,
    AUTHORIZATION_CHECK_LANE_BUDGET, AUTH_REFRESH_ATTEMPT_BUDGET, AUTH_REISSUE_ATTEMPT_BUDGET,
    AUTH_RENEWAL_PASS_BUDGET, AUTH_SWEEP_BATCH_SIZE, AUTH_SWEEP_TWO_LANE_BATCH_SIZE,
    CREDENTIAL_ROTATION_AGE,
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
    // what says whose it was. Reclamation resolves the owner's current bearer
    // from durable custody after restart. It lacks that authority only when a
    // non-renewable access token has expired (or the provider has withdrawn
    // it), not merely because this process restarted. New keys then remain
    // valid only to their provider expiry, while legacy keys may have no bound
    // at all. This sweep is the path that takes both back promptly, so a pass
    // it cannot make is still a credential open, not a tidy-up postponed.
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

        // Sessions whose own credential is old enough to rotate or inside the
        // provider-expiry selection horizon. Both paths get twelve hours of
        // retry runway; the narrower delivery margin is checked only before a
        // value crosses into a runtime. Legacy values remain age-driven.
        // Distinct from the subject refresh below: that keeps this service able
        // to mint, while this keeps sessions able to act.
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
            let replace_before = credential_replace_before();
            let value = match auth
                .reissue_with_budget(
                    &session,
                    &owner,
                    &services,
                    replace_before,
                    item_budget.clone(),
                )
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
            let prepare_auth = Arc::clone(auth);
            let prepare_session = session.clone();
            let prepare_value = value.clone();
            let prepare_budget = item_budget.clone();
            let commit_auth = Arc::clone(auth);
            let commit_session = session.clone();
            let commit_value = value.clone();
            let commit_budget = item_budget.clone();
            match service
                .push_session_credential_with(
                    &session,
                    &value,
                    move |target| async move {
                        prepare_auth
                            .prepare_reissue_delivery_with_budget(
                                &prepare_session,
                                &prepare_value,
                                &target,
                                replace_before,
                                &prepare_budget,
                            )
                            .await
                    },
                    move |target| async move {
                        commit_auth
                            .confirm_reissue_delivery_with_budget(
                                &commit_session,
                                &commit_value,
                                &target,
                                replace_before,
                                &commit_budget,
                            )
                            .await
                    },
                )
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
                        "credential replacement failed during coordination or durable custody finalization"
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
    use layer2_core::{
        CredentialDeliveryTarget, CredentialSource as _, ServiceId, SessionId, SubjectId,
    };
    use std::collections::HashSet;
    use std::time::SystemTime;

    #[derive(Clone, Default)]
    struct TrustedDebtPlatform {
        deleted: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[derive(Clone, Default)]
    struct BudgetMintPlatform {
        mints: Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn list_budget_services(headers: axum::http::HeaderMap) -> axum::Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        axum::Json(serde_json::json!({
            "services": [{
                "id": "resolved-model",
                "slug": "model",
                "is_active": true,
            }]
        }))
    }

    async fn mint_budget_key(
        axum::extract::State(platform): axum::extract::State<BudgetMintPlatform>,
        headers: axum::http::HeaderMap,
    ) -> axum::Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        let mint = platform
            .mints
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        axum::Json(serde_json::json!({
            "id": format!("provider-key-{mint}"),
            "full_key": format!("full-key-{mint}"),
            "scopes": "proxy read",
            "state_version": mint,
            "allowed_service_ids": ["resolved-model"],
            "allowed_node_ids": [],
            "allow_all_services": false,
            "allow_all_nodes": false,
        }))
    }

    async fn list_budget_keys(headers: axum::http::HeaderMap) -> axum::Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        axum::Json(serde_json::json!({ "keys": [] }))
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

    #[tokio::test]
    async fn reissue_budget_covers_binding_delivery_and_promotion() {
        let platform = BudgetMintPlatform::default();
        let app = axum::Router::new()
            .route(
                "/api/v1/user-services",
                axum::routing::get(list_budget_services),
            )
            .route("/api/v1/api-keys", axum::routing::post(mint_budget_key))
            .with_state(platform.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind budget mint platform");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve budget mint platform");
        });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store.clone(),
        );
        auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
        let owner = SubjectId("owner".into());
        auth.store_tokens(
            &owner,
            SubjectTokens {
                access_token: "owner-access".into(),
                refresh_token: None,
                expires_at: std::time::Instant::now() + Duration::from_secs(60),
            },
        )
        .await
        .expect("owner token persists");
        let session = SessionId("sesn_expired_pending_budget".into());
        let services = [ServiceId::new("model")];
        assert_eq!(
            auth.credential_for(&session, &owner, &services, None)
                .await
                .expect("initial A mints"),
            Some("full-key-1".into())
        );
        assert_eq!(
            auth.reissue(&session, &owner, &services)
                .await
                .expect("pending B mints"),
            Some("full-key-2".into())
        );
        let mut custody = store
            .credential(&session)
            .await
            .expect("store works")
            .expect("A and B remain in custody");
        let generation = custody.generation;
        custody.generation += 1;
        custody
            .pending
            .as_mut()
            .expect("B remains pending")
            .expires_at = Some(SystemTime::now() - Duration::from_secs(1));
        assert!(store
            .replace_credential(generation, custody)
            .await
            .expect("B expiry persists"));

        let budget = AuthOperationBudget::standalone(AUTH_REISSUE_ATTEMPT_BUDGET);
        assert!(budget.take(2), "session context fits the item allowance");
        let replace_before = credential_replace_before();
        let value = auth
            .reissue_with_budget(&session, &owner, &services, replace_before, budget.clone())
            .await
            .expect("stale B retires and fresh C mints")
            .expect("C is selected");
        assert_eq!(value, "full-key-3");
        assert!(budget.take(1), "runtime delivery fits the item allowance");
        let target = CredentialDeliveryTarget {
            runtime_generation: 2,
            runtime_id: "runtime-h2".into(),
        };
        assert!(auth
            .prepare_reissue_delivery_with_budget(
                &session,
                &value,
                &target,
                replace_before,
                &budget,
            )
            .await
            .expect("C binds to H2"));
        assert!(auth
            .confirm_reissue_delivery_with_budget(
                &session,
                &value,
                &target,
                replace_before,
                &budget,
            )
            .await
            .expect("H2 acknowledgement promotes C"));
        assert_eq!(budget.remaining(), 0);
        assert!(!budget.take(1));

        let settled = store
            .credential(&session)
            .await
            .expect("store works")
            .expect("custody remains");
        assert_eq!(
            settled.current.as_ref().map(|held| held.id.as_str()),
            Some("provider-key-3")
        );
        assert!(settled.pending.is_none());
        assert!(settled.pending_target.is_none());
        assert_eq!(
            settled
                .revoke_debt
                .iter()
                .map(|debt| debt.id.as_str())
                .collect::<Vec<_>>(),
            ["provider-key-1", "provider-key-2"]
        );
        assert_eq!(platform.mints.load(std::sync::atomic::Ordering::SeqCst), 3);
        server.abort();
    }

    #[tokio::test]
    async fn recurring_selector_rotates_a_young_near_expiry_current_through_acknowledgement() {
        let runtime = crate::pty_integration_tests::credential_sweep_fixture().await;
        let session = runtime.session.clone();
        let owner = runtime.owner.clone();
        let services = [ServiceId::new("model")];
        let platform = BudgetMintPlatform::default();
        let app = axum::Router::new()
            .route(
                "/api/v1/user-services",
                axum::routing::get(list_budget_services),
            )
            .route(
                "/api/v1/api-keys",
                axum::routing::get(list_budget_keys).post(mint_budget_key),
            )
            .with_state(platform.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind near-expiry mint platform");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve near-expiry mint platform");
        });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store.clone(),
        );
        auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
        auth.store_tokens(
            &owner,
            SubjectTokens {
                access_token: "owner-access".into(),
                refresh_token: None,
                expires_at: std::time::Instant::now() + Duration::from_secs(60),
            },
        )
        .await
        .expect("owner token persists");
        assert_eq!(
            auth.credential_for(&session, &owner, &services, None)
                .await
                .expect("initial A mints"),
            Some("full-key-1".into())
        );
        assert!(
            runtime
                .service
                .push_session_credential(&session, "full-key-1".into())
                .await
        );
        let initially_delivered = runtime.delivered_credentials().await;
        assert_eq!(initially_delivered.len(), 1);
        assert_eq!(initially_delivered[0].1, "full-key-1");
        let runtime_handle = initially_delivered[0].0.clone();

        let original_expires_at = SystemTime::now() + Duration::from_secs(4 * 60);
        let mut custody = store
            .credential(&session)
            .await
            .expect("store works")
            .expect("A remains in custody");
        let generation = custody.generation;
        custody.generation += 1;
        let current = custody.current.as_mut().expect("A is current");
        current.issued_at = SystemTime::now();
        current.expires_at = Some(original_expires_at);
        assert!(store
            .replace_credential(generation, custody)
            .await
            .expect("near-expiry A persists"));
        platform.mints.store(2, std::sync::atomic::Ordering::SeqCst);

        let _operations = sweep_once(&auth, &runtime.service).await;

        let delivered = runtime.delivered_credentials().await;
        assert_eq!(
            delivered,
            vec![
                (runtime_handle.clone(), "full-key-1".into()),
                (runtime_handle, "full-key-3".into()),
            ],
            "the recurring sweep installs C on the same live runtime that held A"
        );

        let settled = store
            .credential(&session)
            .await
            .expect("store works")
            .expect("custody remains");
        assert_eq!(
            settled.current.as_ref().map(|held| held.id.as_str()),
            Some("provider-key-3")
        );
        assert!(settled.pending.is_none());
        assert!(settled.pending_target.is_none());
        assert_eq!(
            settled
                .revoke_debt
                .iter()
                .map(|debt| debt.id.as_str())
                .collect::<Vec<_>>(),
            ["provider-key-1"]
        );
        assert!(
            SystemTime::now() < original_expires_at,
            "A enters durable revoke debt before its provider expiry"
        );
        server.abort();
    }

    #[tokio::test]
    async fn age_expiry_overlap_consumes_one_attempt_allowance_and_malformed_rows_consume_none() {
        let owner = SubjectId("owner".into());
        let session = SessionId("sesn_age_expiry_overlap".into());
        let overlapping_state = || SessionCredentialState {
            session: session.clone(),
            generation: 0,
            request_bound_credential: None,
            current: Some(DurableCredential {
                id: "provider-overlap".into(),
                value: SecretValue::new("value".into()),
                owner: owner.clone(),
                issued_at: SystemTime::now() - CREDENTIAL_ROTATION_AGE - Duration::from_secs(60),
                expires_at: Some(SystemTime::now() + Duration::from_secs(60)),
                scope_fingerprint: "scope".into(),
                custody_generation: 0,
                provider_state_version: 0,
                grant_fingerprint: String::new(),
                revocation_credential: None,
            }),
            pending: None,
            pending_target: None,
            revoke_debt: Vec::new(),
            lease: None,
            retired: false,
        };

        let store = Arc::new(MemoryAuthStore::default());
        assert!(store
            .insert_credential(overlapping_state())
            .await
            .expect("overlapping state persists"));
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store,
        );
        let budget = AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE);
        let due = auth
            .sessions_due_for_reissue_with_budget(CREDENTIAL_ROTATION_AGE, &budget)
            .await
            .expect("overlapping raw lanes are read");
        assert_eq!(due.work, vec![session.clone()]);
        assert!(due.first_error.is_none());
        assert_eq!(
            budget.remaining(),
            AUTH_SWEEP_BATCH_SIZE - (1 + 6 + 2 + AUTH_REISSUE_ATTEMPT_BUDGET),
            "both raw index rows and two empty spill reads are charged, but the replacement attempt is reserved once"
        );

        let malformed_store = Arc::new(MemoryAuthStore::default());
        assert!(malformed_store
            .insert_credential(overlapping_state())
            .await
            .expect("malformed fixture persists before read failure injection"));
        malformed_store
            .fail_credential_read(Some(session.clone()))
            .await;
        let malformed_auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            malformed_store,
        );
        let malformed_budget = AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE);
        let malformed = malformed_auth
            .sessions_due_for_reissue_with_budget(CREDENTIAL_ROTATION_AGE, &malformed_budget)
            .await
            .expect("per-row decode failures remain isolated");
        assert!(malformed.work.is_empty());
        assert!(malformed.first_error.is_some());
        assert_eq!(
            malformed_budget.remaining(),
            AUTH_SWEEP_BATCH_SIZE - (1 + 6 + 2),
            "malformed duplicate rows and empty spill reads consume cost but no attempt allowance"
        );
    }

    #[tokio::test]
    async fn spill_page_age_expiry_overlap_prepays_one_unique_attempt() {
        let store = Arc::new(MemoryAuthStore::default());
        let owner = SubjectId("owner".into());
        let overlap = SessionId("sesn_spill_overlap".into());
        let expiry_first = SessionId("sesn_spill_expiry_first".into());
        let now = SystemTime::now();
        let state = |session: SessionId, issued_at, expires_at| SessionCredentialState {
            session,
            generation: 0,
            request_bound_credential: None,
            current: Some(DurableCredential {
                id: "provider-overlap".into(),
                value: SecretValue::new("value".into()),
                owner: owner.clone(),
                issued_at,
                expires_at: Some(expires_at),
                scope_fingerprint: "scope".into(),
                custody_generation: 0,
                provider_state_version: 0,
                grant_fingerprint: String::new(),
                revocation_credential: None,
            }),
            pending: None,
            pending_target: None,
            revoke_debt: Vec::new(),
            lease: None,
            retired: false,
        };
        assert!(store
            .insert_credential(state(
                expiry_first.clone(),
                now,
                now + Duration::from_secs(60),
            ))
            .await
            .expect("expiry-only first row persists"));
        assert!(store
            .insert_credential(state(
                overlap.clone(),
                now - CREDENTIAL_ROTATION_AGE - Duration::from_secs(60),
                now + Duration::from_secs(2 * 60),
            ))
            .await
            .expect("overlapping second expiry row persists"));
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store,
        );
        let budget = AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE);
        let due = auth
            .sessions_due_for_reissue_with_budget(CREDENTIAL_ROTATION_AGE, &budget)
            .await
            .expect("spill page loads");
        assert_eq!(due.work.len(), 2);
        assert!(due.work.contains(&overlap));
        assert!(due.work.contains(&expiry_first));
        assert!(due.first_error.is_none());
        assert_eq!(
            budget.remaining(),
            AUTH_SWEEP_BATCH_SIZE - (1 + 6 + 3 + 2 * AUTH_REISSUE_ATTEMPT_BUDGET),
            "the overlap costs two raw rows across pages but one unique attempt"
        );
    }

    #[tokio::test]
    async fn sole_expiry_burst_spills_capacity_with_hours_of_retry_runway() {
        const BURST: usize = 6;

        let store = Arc::new(MemoryAuthStore::default());
        let owner = SubjectId("owner".into());
        let expires_at = SystemTime::now() + CREDENTIAL_ROTATION_AGE - Duration::from_secs(60);
        for number in 0..BURST {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("expiry-burst-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(DurableCredential {
                        id: format!("provider-expiry-burst-{number:03}"),
                        value: SecretValue::new("value".into()),
                        owner: owner.clone(),
                        issued_at: SystemTime::now(),
                        expires_at: Some(expires_at),
                        scope_fingerprint: "scope".into(),
                        custody_generation: 0,
                        provider_state_version: 0,
                        grant_fingerprint: String::new(),
                        revocation_credential: None,
                    }),
                    pending: None,
                    pending_target: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("expiry burst row persists"));
        }
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store,
        );

        let first_budget = AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE);
        let first = auth
            .sessions_due_for_reissue_with_budget(CREDENTIAL_ROTATION_AGE, &first_budget)
            .await
            .expect("first burst page loads");
        assert_eq!(first.work.len(), 4, "three unused lanes spill to expiry");
        let second_budget = AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE);
        let second = auth
            .sessions_due_for_reissue_with_budget(CREDENTIAL_ROTATION_AGE, &second_budget)
            .await
            .expect("second burst page loads");
        assert_eq!(second.work.len(), BURST - first.work.len());
        let selected = first
            .work
            .into_iter()
            .chain(second.work)
            .collect::<HashSet<_>>();
        assert_eq!(selected.len(), BURST);
        assert!(
            SystemTime::now() < expires_at,
            "the burst is admitted far outside the five-minute delivery margin"
        );
    }

    #[tokio::test]
    async fn expiry_crossing_frozen_horizon_appears_after_epoch_wrap() {
        let store = Arc::new(MemoryAuthStore::default());
        let owner = SubjectId("owner".into());
        let now = SystemTime::now();
        for number in 0..4 {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("frozen-expiry-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(DurableCredential {
                        id: format!("provider-frozen-expiry-{number:03}"),
                        value: SecretValue::new("value".into()),
                        owner: owner.clone(),
                        issued_at: now,
                        expires_at: Some(now + Duration::from_secs(60)),
                        scope_fingerprint: "scope".into(),
                        custody_generation: 0,
                        provider_state_version: 0,
                        grant_fingerprint: String::new(),
                        revocation_credential: None,
                    }),
                    pending: None,
                    pending_target: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("frozen epoch row persists"));
        }
        let crossing = SessionId("frozen-expiry-crossing".into());
        assert!(store
            .insert_credential(SessionCredentialState {
                session: crossing.clone(),
                generation: 0,
                request_bound_credential: None,
                current: Some(DurableCredential {
                    id: "provider-frozen-expiry-crossing".into(),
                    value: SecretValue::new("value".into()),
                    owner,
                    issued_at: now,
                    expires_at: Some(
                        SystemTime::now() + CREDENTIAL_ROTATION_AGE + Duration::from_secs(1),
                    ),
                    scope_fingerprint: "scope".into(),
                    custody_generation: 0,
                    provider_state_version: 0,
                    grant_fingerprint: String::new(),
                    revocation_credential: None,
                }),
                pending: None,
                pending_target: None,
                revoke_debt: Vec::new(),
                lease: None,
                retired: false,
            })
            .await
            .expect("crossing row persists"));
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
            store,
        );

        let first = auth
            .sessions_due_for_reissue_with_budget(
                CREDENTIAL_ROTATION_AGE,
                &AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE),
            )
            .await
            .expect("the first frozen page loads");
        assert_eq!(first.work.len(), 4);
        assert!(!first.work.contains(&crossing));
        tokio::time::sleep(Duration::from_millis(1_100)).await;

        let old_epoch_tail = auth
            .sessions_due_for_reissue_with_budget(
                CREDENTIAL_ROTATION_AGE,
                &AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE),
            )
            .await
            .expect("the old frozen epoch completes");
        assert!(old_epoch_tail.work.is_empty());
        let wrapped_head = auth
            .sessions_due_for_reissue_with_budget(
                CREDENTIAL_ROTATION_AGE,
                &AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE),
            )
            .await
            .expect("the refreshed expiry epoch head loads");
        assert_eq!(wrapped_head.work.len(), 4);
        assert!(!wrapped_head.work.contains(&crossing));
        let wrapped_tail = auth
            .sessions_due_for_reissue_with_budget(
                CREDENTIAL_ROTATION_AGE,
                &AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE),
            )
            .await
            .expect("the refreshed expiry epoch tail loads");
        assert_eq!(wrapped_tail.work, vec![crossing]);
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
        let old_issued_at = SystemTime::now() - CREDENTIAL_ROTATION_AGE - Duration::from_secs(60);
        let young_issued_at = SystemTime::now();
        let credential = |id: String, issued_at| DurableCredential {
            id,
            value: SecretValue::new("credential-value".into()),
            owner: owner.clone(),
            issued_at,
            expires_at: None,
            scope_fingerprint: "scope".into(),
            custody_generation: 0,
            provider_state_version: 0,
            grant_fingerprint: String::new(),
            revocation_credential: None,
        };
        let lane_work = crate::auth::AUTH_SWEEP_BATCH_SIZE / 4 + 2;
        for number in 0..lane_work {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("current-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(
                        format!("provider-current-{number:03}"),
                        old_issued_at,
                    )),
                    pending: None,
                    pending_target: None,
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
                    current: Some(credential(
                        format!("provider-old-{number:03}"),
                        old_issued_at,
                    )),
                    pending: Some(credential(
                        format!("provider-pending-{number:03}"),
                        old_issued_at,
                    )),
                    pending_target: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("pending state persists"));

            let mut expiring =
                credential(format!("provider-expiring-{number:03}"), young_issued_at);
            expiring.expires_at = Some(SystemTime::now() + Duration::from_secs(60));
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("expiring-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(expiring),
                    pending: None,
                    pending_target: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("expiring current state persists"));

            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("expired-lease-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(
                        format!("provider-leased-{number:03}"),
                        old_issued_at,
                    )),
                    pending: None,
                    pending_target: None,
                    revoke_debt: Vec::new(),
                    lease: Some(CredentialLease {
                        holder: "abandoned-worker".into(),
                        fence: 1,
                        acquired_at: old_issued_at,
                        expires_at: SystemTime::now() - Duration::from_secs(1),
                        scope_fingerprint: "scope".into(),
                    }),
                    retired: false,
                })
                .await
                .expect("expired lease state persists"));
        }

        let first = auth
            .sessions_due_for_reissue(CREDENTIAL_ROTATION_AGE)
            .await
            .expect("first mixed page loads");
        assert_eq!(first.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("current-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 4
        );
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("pending-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 4
        );
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("expiring-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 4
        );
        assert_eq!(
            first
                .iter()
                .filter(|session| session.as_str().starts_with("expired-lease-"))
                .count(),
            crate::auth::AUTH_SWEEP_BATCH_SIZE / 4
        );

        for number in 0..crate::auth::AUTH_SWEEP_BATCH_SIZE {
            assert!(store
                .insert_credential(SessionCredentialState {
                    session: SessionId(format!("pending-tail-{number:03}")),
                    generation: 0,
                    request_bound_credential: None,
                    current: Some(credential(
                        format!("provider-tail-old-{number:03}"),
                        old_issued_at,
                    )),
                    pending: Some(credential(
                        format!("provider-tail-pending-{number:03}"),
                        old_issued_at,
                    )),
                    pending_target: None,
                    revoke_debt: Vec::new(),
                    lease: None,
                    retired: false,
                })
                .await
                .expect("growing pending tail persists"));
        }

        let second = auth
            .sessions_due_for_reissue(CREDENTIAL_ROTATION_AGE)
            .await
            .expect("second mixed page loads");
        assert_eq!(second.len(), 8);
        assert_eq!(
            second
                .iter()
                .filter(|session| session.as_str().starts_with("current-"))
                .count(),
            2
        );

        let wrapped = auth
            .sessions_due_for_reissue(CREDENTIAL_ROTATION_AGE)
            .await
            .expect("finite epochs wrap despite a sustained tail");
        assert_eq!(wrapped.len(), crate::auth::AUTH_SWEEP_BATCH_SIZE);
        assert!(wrapped
            .iter()
            .any(|session| session.as_str() == "pending-000"));
        assert_eq!(
            second
                .iter()
                .filter(|session| session.as_str().starts_with("expiring-"))
                .count(),
            2
        );
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
            custody_generation: 0,
            provider_state_version: 0,
            grant_fingerprint: String::new(),
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
                    pending_target: None,
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
                    pending_target: None,
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
        const PASSES: usize = 3;
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
            custody_generation: 0,
            provider_state_version: 0,
            grant_fingerprint: String::new(),
            revocation_credential: None,
        };
        for number in 0..REISSUE_PER_SUBLANE {
            for (kind, pending, lease, expiring) in [
                ("current", false, false, false),
                ("expiring", false, false, true),
                ("pending", true, false, false),
                ("lease", false, true, false),
            ] {
                let session = SessionId(format!("{kind}-{number:03}"));
                let mut current = credential(format!("provider-{kind}-{number:03}"));
                if expiring {
                    current.issued_at = SystemTime::now() + Duration::from_secs(60 * 60);
                    current.expires_at = Some(SystemTime::now() + Duration::from_secs(60));
                }
                assert!(store
                    .insert_credential(SessionCredentialState {
                        session,
                        generation: 0,
                        request_bound_credential: None,
                        current: Some(current),
                        pending: pending
                            .then(|| credential(format!("provider-pending-{number:03}"))),
                        pending_target: None,
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
        for _ in 0..PASSES {
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

        // Target binding makes each reissue attempt costlier, so the fixed hard
        // lane ceiling now admits one row from each sublane on every pass.
        assert_eq!(reissued.len(), PASSES * 4);
        assert_eq!(refreshed.len(), REFRESH_PER_SUBLANE * 2);
        for prefix in ["current-", "expiring-", "pending-", "lease-"] {
            assert_eq!(
                reissued
                    .iter()
                    .filter(|session| session.starts_with(prefix))
                    .count(),
                PASSES
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
                pending_target: None,
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
