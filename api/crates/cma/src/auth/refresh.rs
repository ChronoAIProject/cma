//! Exchanging a refresh token for a new access token.

use std::time::{Duration, SystemTime};

use layer2_core::store::StoreError;
use layer2_core::SubjectId;
use serde::Deserialize;

use super::persistence::{DurableSubjectTokens, SecretValue, SubjectRefreshLease};
use super::{random_token, AuthMode, AuthState};

const REFRESH_LEASE_LIFETIME: Duration = Duration::from_secs(45);
const REFRESH_LEASE_POLL: Duration = Duration::from_millis(250);

enum LeaseOutcome {
    Acquired(String),
    AlreadyRefreshed(String),
}

impl AuthState {
    async fn acquire_refresh_lease(
        &self,
        subject: &SubjectId,
        requested_refresh: &str,
    ) -> Result<LeaseOutcome, StoreError> {
        loop {
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
            if self.store.replace_subject(current.revision, next).await? {
                return Ok(LeaseOutcome::Acquired(holder));
            }
        }
    }

    async fn finish_refresh_lease(
        &self,
        subject: &SubjectId,
        holder: &str,
        tokens: Option<DurableSubjectTokens>,
    ) -> Result<bool, StoreError> {
        loop {
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
            next.tokens = tokens.clone();
            next.refresh_lease = None;
            if self.store.replace_subject(current.revision, next).await? {
                return Ok(true);
            }
        }
    }

    async fn abandon_refresh_lease(
        &self,
        subject: &SubjectId,
        holder: &str,
    ) -> Result<(), StoreError> {
        loop {
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
            if self.store.replace_subject(current.revision, next).await? {
                return Ok(());
            }
        }
    }

    /// Exchange a refresh token for a new access token.
    ///
    /// The durable lease serializes rotating refresh-token exchanges across
    /// replicas. A provider refusal removes delegation only while this worker
    /// still owns that lease; it can never erase a newer successful refresh.
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
        let holder = match self
            .acquire_refresh_lease(subject, refresh_token)
            .await
            .map_err(|error| format!("auth store failed before refresh: {error}"))?
        {
            LeaseOutcome::Acquired(holder) => holder,
            LeaseOutcome::AlreadyRefreshed(access) => return Ok(access),
        };

        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];
        let resp = match self
            .http
            .post(&meta.token_endpoint)
            .form(&form)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.abandon_refresh_lease(subject, &holder)
                    .await
                    .map_err(|store| format!("auth store failed after refresh request: {store}"))?;
                return Err(format!("refresh request failed: {error}"));
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            self.finish_refresh_lease(subject, &holder, None)
                .await
                .map_err(|error| format!("auth store failed after rejected refresh: {error}"))?;
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
        let body: RefreshResponse = match resp.json().await {
            Ok(body) => body,
            Err(error) => {
                self.abandon_refresh_lease(subject, &holder)
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
            .finish_refresh_lease(subject, &holder, Some(tokens))
            .await
            .map_err(|error| format!("auth store failed while persisting refresh: {error}"))?
        {
            return Ok(access_token);
        }

        // A lease can be lost only after expiry. Return the durable winner's
        // access value rather than the stale provider response this worker no
        // longer owns.
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

    use axum::{extract::State, routing::post, Json, Router};

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

        let store = Arc::new(MemoryAuthStore::default());
        let mode = AuthMode::Provider {
            base_url: base.clone(),
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

        let one = tokio::spawn({
            let auth = Arc::clone(&first);
            let subject = subject.clone();
            async move { auth.refresh(&subject, "refresh-old").await }
        });
        let two = tokio::spawn({
            let auth = Arc::clone(&second);
            let subject = subject.clone();
            async move { auth.refresh(&subject, "refresh-old").await }
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
}
