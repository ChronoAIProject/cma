//! Exchanging a refresh token for a new access token.

use std::time::{Duration, SystemTime};

use layer2_core::store::StoreError;
use layer2_core::SubjectId;
use serde::Deserialize;

use super::persistence::{DurableSubjectTokens, SecretValue, SubjectRefreshLease};
use super::{next_token_generation, random_token, AuthMode, AuthOperationBudget, AuthState};

const REFRESH_LEASE_LIFETIME: Duration = Duration::from_secs(45);
const REFRESH_LEASE_POLL: Duration = Duration::from_millis(250);

enum LeaseOutcome {
    Acquired(String),
    AlreadyRefreshed(String),
    Deferred,
}

fn take_operation(
    budget: Option<&AuthOperationBudget>,
    amount: usize,
    operation: &str,
) -> Result<(), StoreError> {
    if budget.is_some_and(|budget| !budget.take(amount)) {
        return Err(StoreError::Unavailable(format!(
            "recurring authentication operation budget exhausted before {operation}"
        )));
    }
    Ok(())
}

impl AuthState {
    async fn acquire_refresh_lease(
        &self,
        subject: &SubjectId,
        requested_refresh: &str,
        requested_incarnation: &str,
        requested_generation: u64,
        budget: Option<&AuthOperationBudget>,
    ) -> Result<LeaseOutcome, StoreError> {
        loop {
            take_operation(budget, 1, "subject refresh lease read")?;
            let Some(current) = self.store.subject(subject).await? else {
                return Err(StoreError::Malformed(
                    "cannot refresh an unknown subject".into(),
                ));
            };
            let Some(tokens) = current.tokens.as_ref() else {
                return Err(StoreError::Malformed(
                    "subject has no refreshable delegation".into(),
                ));
            };
            if current.incarnation != requested_incarnation
                || current.token_generation != requested_generation
            {
                return Ok(LeaseOutcome::AlreadyRefreshed(
                    tokens.access_token.expose().to_string(),
                ));
            }
            let Some(stored_refresh) = tokens.refresh_token.as_ref() else {
                return Err(StoreError::Malformed("subject has no refresh token".into()));
            };
            if stored_refresh.expose() != requested_refresh {
                return Ok(LeaseOutcome::AlreadyRefreshed(
                    tokens.access_token.expose().to_string(),
                ));
            }
            if current
                .refresh_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > SystemTime::now())
            {
                if budget.is_some() {
                    return Ok(LeaseOutcome::Deferred);
                }
                tokio::time::sleep(REFRESH_LEASE_POLL).await;
                continue;
            }

            let holder = random_token();
            let mut next = current.clone();
            next.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("subject auth revision exhausted".into()))?;
            next.refresh_lease = Some(SubjectRefreshLease {
                holder: holder.clone(),
                expires_at: SystemTime::now() + REFRESH_LEASE_LIFETIME,
            });
            take_operation(budget, 2, "subject refresh lease write")?;
            if self
                .store
                .replace_subject(&current.incarnation, current.revision, next)
                .await?
            {
                return Ok(LeaseOutcome::Acquired(holder));
            }
        }
    }

    async fn finish_refresh_lease(
        &self,
        subject: &SubjectId,
        holder: &str,
        tokens: Option<DurableSubjectTokens>,
        budget: Option<&AuthOperationBudget>,
    ) -> Result<bool, StoreError> {
        loop {
            take_operation(budget, 1, "subject refresh completion read")?;
            let Some(current) = self.store.subject(subject).await? else {
                return Ok(false);
            };
            if current
                .refresh_lease
                .as_ref()
                .is_none_or(|lease| lease.holder != holder)
            {
                return Ok(false);
            }
            let mut next = current.clone();
            next.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("subject auth revision exhausted".into()))?;
            next.token_generation = next_token_generation(current.token_generation)?;
            next.tokens = tokens.clone();
            if tokens.is_none() {
                next.authorization = layer2_core::Authorization::none();
            }
            next.refresh_lease = None;
            take_operation(budget, 2, "subject refresh completion write")?;
            if self
                .store
                .replace_subject(&current.incarnation, current.revision, next)
                .await?
            {
                return Ok(true);
            }
        }
    }

    async fn abandon_refresh_lease(
        &self,
        subject: &SubjectId,
        holder: &str,
        budget: Option<&AuthOperationBudget>,
    ) -> Result<(), StoreError> {
        loop {
            take_operation(budget, 1, "subject refresh abandonment read")?;
            let Some(current) = self.store.subject(subject).await? else {
                return Ok(());
            };
            if current
                .refresh_lease
                .as_ref()
                .is_none_or(|lease| lease.holder != holder)
            {
                return Ok(());
            }
            let mut next = current.clone();
            next.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("subject auth revision exhausted".into()))?;
            next.refresh_lease = None;
            take_operation(budget, 2, "subject refresh abandonment write")?;
            if self
                .store
                .replace_subject(&current.incarnation, current.revision, next)
                .await?
            {
                return Ok(());
            }
        }
    }

    /// Exchange a refresh token for a new access token.
    ///
    /// The durable lease and observed token generation serialize refresh-token
    /// exchanges across replicas, including providers that reuse one refresh
    /// token. A provider refusal removes delegation only while this worker still
    /// owns that lease; it can never erase a newer successful refresh.
    #[cfg(test)]
    pub async fn refresh(
        &self,
        subject: &SubjectId,
        refresh_token: &str,
        incarnation: &str,
        token_generation: u64,
    ) -> Result<String, String> {
        self.refresh_inner(subject, refresh_token, incarnation, token_generation, None)
            .await
    }

    pub(crate) async fn refresh_with_budget(
        &self,
        subject: &SubjectId,
        refresh_token: &str,
        incarnation: &str,
        token_generation: u64,
        budget: &AuthOperationBudget,
    ) -> Result<String, String> {
        self.refresh_inner(
            subject,
            refresh_token,
            incarnation,
            token_generation,
            Some(budget),
        )
        .await
    }

    async fn refresh_inner(
        &self,
        subject: &SubjectId,
        refresh_token: &str,
        incarnation: &str,
        token_generation: u64,
        budget: Option<&AuthOperationBudget>,
    ) -> Result<String, String> {
        let AuthMode::Provider {
            client_id,
            client_secret,
            ..
        } = &self.mode
        else {
            return Err("no provider configured".into());
        };
        take_operation(budget, 1, "provider metadata read").map_err(|error| error.to_string())?;
        let meta = self.metadata().await?;
        let holder = match self
            .acquire_refresh_lease(
                subject,
                refresh_token,
                incarnation,
                token_generation,
                budget,
            )
            .await
            .map_err(|error| format!("auth store failed before refresh: {error}"))?
        {
            LeaseOutcome::Acquired(holder) => holder,
            LeaseOutcome::AlreadyRefreshed(access) => return Ok(access),
            LeaseOutcome::Deferred => return Err("subject refresh is already leased".into()),
        };

        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];
        take_operation(budget, 1, "provider token refresh").map_err(|error| error.to_string())?;
        let resp = match self
            .http
            .post(&meta.token_endpoint)
            .form(&form)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.abandon_refresh_lease(subject, &holder, budget)
                    .await
                    .map_err(|store| format!("auth store failed after refresh request: {store}"))?;
                return Err(format!("refresh request failed: {error}"));
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            #[derive(Deserialize)]
            struct OAuthErrorResponse {
                error: String,
            }
            let error_code = match resp.bytes().await {
                Ok(body) => serde_json::from_slice::<OAuthErrorResponse>(&body)
                    .ok()
                    .map(|body| body.error),
                Err(_) => None,
            };
            // Transport status has precedence over an OAuth-shaped body. A
            // throttling response or provider outage cannot prove the grant
            // is invalid, even when a proxy or provider emits that code.
            let terminal_invalid_grant = status == reqwest::StatusCode::BAD_REQUEST
                && error_code.as_deref() == Some("invalid_grant");
            if terminal_invalid_grant {
                self.finish_refresh_lease(subject, &holder, None, budget)
                    .await
                    .map_err(|error| {
                        format!("auth store failed after terminal refresh rejection: {error}")
                    })?;
            } else {
                // HTTP throttling and provider outages say nothing about the
                // refresh token. The same is true of client/configuration
                // errors and unparseable responses: only invalid_grant is an
                // OAuth assertion that this subject's grant is no longer usable.
                self.abandon_refresh_lease(subject, &holder, budget)
                    .await
                    .map_err(|error| {
                        format!("auth store failed after retryable refresh rejection: {error}")
                    })?;
            }
            return Err(match error_code {
                Some(code) => format!("provider refused refresh with {status} ({code})"),
                None => format!("provider refused refresh with {status}"),
            });
        }

        #[derive(Deserialize)]
        struct RefreshResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            expires_in: Option<u64>,
        }
        let body: RefreshResponse = match resp.json().await {
            Ok(body) => body,
            Err(error) => {
                self.abandon_refresh_lease(subject, &holder, budget)
                    .await
                    .map_err(|store| {
                        format!("auth store failed after refresh response: {store}")
                    })?;
                return Err(format!("refresh response not usable: {error}"));
            }
        };
        let access_token = body.access_token.clone();
        let tokens = DurableSubjectTokens {
            access_token: SecretValue::new(body.access_token),
            // A provider that rotates returns a new value; otherwise the value
            // whose lease we hold remains the next refresh credential.
            refresh_token: Some(SecretValue::new(
                body.refresh_token
                    .unwrap_or_else(|| refresh_token.to_string()),
            )),
            expires_at: SystemTime::now() + Duration::from_secs(body.expires_in.unwrap_or(3600)),
        };
        if self
            .finish_refresh_lease(subject, &holder, Some(tokens), budget)
            .await
            .map_err(|error| format!("auth store failed while persisting refresh: {error}"))?
        {
            return Ok(access_token);
        }

        // A lease can be lost only after expiry. Return the durable winner's
        // access value rather than the stale provider response this worker no
        // longer owns.
        take_operation(budget, 1, "refresh winner access-token read")
            .map_err(|error| error.to_string())?;
        self.access_token(subject)
            .await
            .map_err(|error| format!("auth store failed after refresh lease loss: {error}"))?
            .ok_or_else(|| "refresh lease was lost without a durable token".into())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};

    use super::*;
    use crate::auth::persistence::{AuthStore, MemoryAuthStore};
    use crate::auth::{ProviderMetadata, SubjectTokens};

    async fn rotate(State(calls): State<Arc<AtomicUsize>>) -> Json<serde_json::Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        Json(serde_json::json!({
            "access_token": "access-new",
            "refresh_token": "refresh-new",
            "expires_in": 3600,
        }))
    }

    async fn reuse_refresh_token(State(calls): State<Arc<AtomicUsize>>) -> Json<serde_json::Value> {
        let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
        Json(serde_json::json!({
            "access_token": format!("access-new-{call}"),
            "expires_in": 3600,
        }))
    }

    async fn reject_refresh(
        State(status): State<StatusCode>,
    ) -> (StatusCode, Json<serde_json::Value>) {
        (
            status,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
    }

    async fn provider_replicas(
        base: &str,
    ) -> (Arc<MemoryAuthStore>, Arc<AuthState>, Arc<AuthState>) {
        let store = Arc::new(MemoryAuthStore::default());
        let mode = AuthMode::Provider {
            base_url: base.to_string(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        };
        let first = AuthState::with_store(mode.clone(), None, store.clone());
        let second = AuthState::with_store(mode, None, store.clone());
        let metadata = ProviderMetadata {
            authorization_endpoint: "http://browser.invalid/authorize".into(),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: None,
            introspection_endpoint: None,
        };
        *first.metadata.write().await = Some(metadata.clone());
        *second.metadata.write().await = Some(metadata);
        (store, first, second)
    }

    #[tokio::test]
    async fn two_replicas_serialize_one_rotating_refresh_exchange() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/token", post(rotate))
            .with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve provider");
        });

        let (store, first, second) = provider_replicas(&base).await;
        let subject = SubjectId("subject-a".into());
        first
            .store_tokens(
                &subject,
                SubjectTokens {
                    access_token: "access-old".into(),
                    refresh_token: Some("refresh-old".into()),
                    expires_at: std::time::Instant::now(),
                },
            )
            .await
            .expect("tokens persist");
        let observed = store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("subject exists");
        let token_generation = observed.token_generation;
        let incarnation = observed.incarnation;

        let one = tokio::spawn({
            let auth = Arc::clone(&first);
            let subject = subject.clone();
            let incarnation = incarnation.clone();
            async move {
                auth.refresh(&subject, "refresh-old", &incarnation, token_generation)
                    .await
            }
        });
        let two = tokio::spawn({
            let auth = Arc::clone(&second);
            let subject = subject.clone();
            async move {
                auth.refresh(&subject, "refresh-old", &incarnation, token_generation)
                    .await
            }
        });
        assert_eq!(
            one.await.expect("first joins").expect("first refreshes"),
            "access-new"
        );
        assert_eq!(
            two.await
                .expect("second joins")
                .expect("second observes winner"),
            "access-new"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let fact = store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("subject exists");
        assert!(fact.refresh_lease.is_none());
        let tokens = fact.tokens.expect("tokens remain");
        assert_eq!(tokens.access_token.expose(), "access-new");
        assert_eq!(
            tokens.refresh_token.as_ref().map(|value| value.expose()),
            Some("refresh-new")
        );
        server.abort();
    }

    #[tokio::test]
    async fn two_replicas_exchange_one_non_rotating_refresh_generation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/token", post(reuse_refresh_token))
            .with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve provider");
        });

        let (store, first, second) = provider_replicas(&base).await;
        let subject = SubjectId("subject-a".into());
        first
            .store_tokens(
                &subject,
                SubjectTokens {
                    access_token: "access-old".into(),
                    refresh_token: Some("refresh-reused".into()),
                    expires_at: std::time::Instant::now(),
                },
            )
            .await
            .expect("tokens persist");

        // Both replicas take the same due-work snapshot before either exchange.
        // Running them sequentially makes the formerly duplicated provider call
        // deterministic: the second snapshot remains stale after the first wins.
        let first_due = first
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("first replica lists due work")
            .pop()
            .expect("subject is due");
        let second_due = second
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("second replica lists due work")
            .pop()
            .expect("subject is due");
        assert_eq!(first_due.token_generation, second_due.token_generation);
        assert_eq!(first_due.incarnation, second_due.incarnation);

        assert_eq!(
            first
                .refresh(
                    &first_due.subject,
                    &first_due.refresh_token,
                    &first_due.incarnation,
                    first_due.token_generation,
                )
                .await
                .expect("first replica refreshes"),
            "access-new-1"
        );
        assert_eq!(
            second
                .refresh(
                    &second_due.subject,
                    &second_due.refresh_token,
                    &second_due.incarnation,
                    second_due.token_generation,
                )
                .await
                .expect("second replica observes the durable winner"),
            "access-new-1"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let fact = store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("subject exists");
        assert_eq!(
            fact.token_generation,
            first_due.token_generation + 1,
            "only the completed token replacement advances the generation"
        );
        assert_eq!(
            fact.tokens
                .expect("tokens remain")
                .refresh_token
                .as_ref()
                .map(SecretValue::expose),
            Some("refresh-reused")
        );
        server.abort();
    }

    #[tokio::test]
    async fn transient_status_precedes_invalid_grant_and_only_bad_request_revokes() {
        let subject = SubjectId("subject-transient".into());
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let app = Router::new()
                .route("/token", post(reject_refresh))
                .with_state(status);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind rejecting provider");
            let base = format!("http://{}", listener.local_addr().expect("local address"));
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("serve rejecting provider");
            });
            let (store, auth, _) = provider_replicas(&base).await;
            auth.store_tokens(
                &subject,
                SubjectTokens {
                    access_token: "access-old".into(),
                    refresh_token: Some("refresh-valid".into()),
                    expires_at: std::time::Instant::now(),
                },
            )
            .await
            .expect("tokens persist");
            let before = store
                .subject(&subject)
                .await
                .expect("store works")
                .expect("subject exists");
            let error = auth
                .refresh(
                    &subject,
                    "refresh-valid",
                    &before.incarnation,
                    before.token_generation,
                )
                .await
                .expect_err("transient rejection is retryable");
            assert!(error.contains(status.as_str()), "got {error}");
            let after = store
                .subject(&subject)
                .await
                .expect("store works")
                .expect("subject remains");
            assert!(after.refresh_lease.is_none());
            assert_eq!(after.token_generation, before.token_generation);
            let tokens = after.tokens.expect("transient response retains tokens");
            assert_eq!(tokens.access_token.expose(), "access-old");
            assert_eq!(
                tokens.refresh_token.as_ref().map(SecretValue::expose),
                Some("refresh-valid")
            );
            server.abort();
        }

        let app = Router::new()
            .route("/token", post(reject_refresh))
            .with_state(StatusCode::BAD_REQUEST);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind terminal provider");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve terminal provider");
        });
        let (store, auth, _) = provider_replicas(&base).await;
        auth.store_tokens(
            &subject,
            SubjectTokens {
                access_token: "access-old".into(),
                refresh_token: Some("refresh-revoked".into()),
                expires_at: std::time::Instant::now(),
            },
        )
        .await
        .expect("tokens persist");
        auth.store_authorization(
            &subject,
            layer2_core::Authorization::from_capabilities([
                layer2_core::Capability::ReadAllSessions,
            ]),
        )
        .await
        .expect("authorization persists");
        let before = store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("subject exists");
        let error = auth
            .refresh(
                &subject,
                "refresh-revoked",
                &before.incarnation,
                before.token_generation,
            )
            .await
            .expect_err("invalid_grant is terminal");
        assert!(error.contains("invalid_grant"), "got {error}");
        let after = store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("subject fact remains");
        assert!(after.tokens.is_none());
        assert!(after.refresh_lease.is_none());
        assert_eq!(after.authorization.capabilities().count(), 0);
        assert_eq!(after.token_generation, before.token_generation + 1);
        server.abort();
    }

    #[tokio::test]
    async fn a_snapshot_from_a_ttl_deleted_incarnation_cannot_refresh_its_replacement() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/token", post(reuse_refresh_token))
            .with_state(Arc::clone(&calls));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind provider");
        let base = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve provider");
        });

        let (expired_store, expired, _) = provider_replicas(&base).await;
        let subject = SubjectId("subject-recreated".into());
        expired
            .store_tokens(
                &subject,
                SubjectTokens {
                    access_token: "access-expired-incarnation".into(),
                    refresh_token: Some("refresh-reused".into()),
                    expires_at: std::time::Instant::now(),
                },
            )
            .await
            .expect("expired incarnation persists");
        let mut expired_fact = expired_store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("expired incarnation exists");
        let expired_revision = expired_fact.revision;
        let expired_incarnation = expired_fact.incarnation.clone();
        expired_fact.revision += 1;
        expired_fact.incarnation = "expired-incarnation".into();
        assert!(expired_store
            .replace_subject(&expired_incarnation, expired_revision, expired_fact)
            .await
            .expect("expired incarnation is deterministic"));
        let stale = expired
            .subjects_needing_refresh(Duration::ZERO)
            .await
            .expect("expired incarnation lists due work")
            .pop()
            .expect("expired incarnation is due");

        // A fresh store represents the same subject after MongoDB's TTL index
        // deleted its former record. Reusing the token text and generation must
        // not make the old work item valid in this new lifecycle.
        let (replacement_store, replacement, _) = provider_replicas(&base).await;
        replacement
            .store_tokens(
                &subject,
                SubjectTokens {
                    access_token: "access-replacement".into(),
                    refresh_token: Some("refresh-reused".into()),
                    expires_at: std::time::Instant::now(),
                },
            )
            .await
            .expect("replacement incarnation persists");
        let mut replacement_fact = replacement_store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("replacement incarnation exists");
        let replacement_revision = replacement_fact.revision;
        let replacement_incarnation = replacement_fact.incarnation.clone();
        replacement_fact.revision += 1;
        replacement_fact.incarnation = "replacement-incarnation".into();
        assert!(replacement_store
            .replace_subject(
                &replacement_incarnation,
                replacement_revision,
                replacement_fact.clone(),
            )
            .await
            .expect("replacement incarnation is deterministic"));
        assert_eq!(stale.token_generation, replacement_fact.token_generation);
        assert_ne!(stale.incarnation, replacement_fact.incarnation);

        assert_eq!(
            replacement
                .refresh(
                    &stale.subject,
                    &stale.refresh_token,
                    &stale.incarnation,
                    stale.token_generation,
                )
                .await
                .expect("stale work observes the replacement"),
            "access-replacement"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(replacement_store
            .subject(&subject)
            .await
            .expect("store works")
            .expect("replacement remains")
            .refresh_lease
            .is_none());
        server.abort();
    }
}
