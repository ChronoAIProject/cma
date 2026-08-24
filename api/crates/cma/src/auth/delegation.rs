//! Handing a session's runtime a credential, and taking it back.
//!
//! Split from the state so that file is about who is signed in and this one
//! about what an agent is allowed to spend on their behalf. They are different
//! questions with different lifetimes: a person stays signed in across many
//! sessions, and a credential is delegated to one runtime and reclaimed when
//! that runtime stops.

use layer2_core::{SessionId, SubjectId};

use super::{AuthState, Issued};

impl AuthState {
    /// Whether an exact key already belongs to the explicit revoke retry path.
    pub(super) async fn revoke_is_queued(&self, id: &str) -> bool {
        self.superseded
            .read()
            .await
            .iter()
            .any(|(queued, _)| queued == id)
    }
}

/// Supply the core with an owner's credential when the owner is absent.
///
/// The core recovers sessions on its own — after a restart, or when a runtime
/// has to be replaced — and a recovered session that presents no credential is
/// a session whose every model call fails. So the credential is fetched from
/// wherever it is currently held rather than being carried along from the
/// request that happened to create the session.
#[async_trait::async_trait]
impl layer2_core::CredentialSource for AuthState {
    async fn credential_for(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[layer2_core::ServiceId],
    ) -> Option<String> {
        let issuer = { self.issuer.read().await.clone() };
        let Some(issuer) = issuer else {
            // No platform configured. The session gets its owner's own token,
            // which is what it got before any of this existed.
            return self.access_token(subject).await;
        };

        // One value per session. Asking again — for a replaced runtime, or
        // because a restart lost the entry — must not leave two credentials
        // alive for one session.
        if let Some(held) = self.issued.read().await.get(session) {
            return Some(held.value.clone());
        }

        let owner_token = self.access_token(subject).await?;
        let minted = issuer
            .mint(session, subject, &owner_token, services)
            .await?;
        self.issued.write().await.insert(
            session.clone(),
            Issued {
                id: minted.id,
                value: minted.full_key.clone(),
                owner: subject.clone(),
                minted_at: std::time::Instant::now(),
            },
        );
        Some(minted.full_key)
    }

    async fn release(&self, session: &SessionId) {
        /*
         * The local ownership transfer belongs to a detached task. Dropping a
         * release future while it waits for a contended queue lock must not
         * cancel the only operation that can turn a terminal session's issued
         * key into retry debt. The task holds every lock in the one global
         * issued -> superseded -> reissues order, then mutates without awaiting.
         */
        let issued = std::sync::Arc::clone(&self.issued);
        let superseded = std::sync::Arc::clone(&self.superseded);
        let reissues = std::sync::Arc::clone(&self.reissues);
        let released_session = session.clone();
        let transfer = tokio::spawn(async move {
            let mut issued = issued.write().await;
            let mut superseded = superseded.write().await;
            let mut progress = reissues.write().await;
            let current = issued.remove(&released_session);
            let replacement = progress.pending.remove(&released_session);
            let minting = progress.minting.contains(&released_session);
            if minting {
                progress.retiring.insert(released_session.clone());
            } else {
                progress.retiring.remove(&released_session);
            }
            let tracked = minting || current.is_some() || replacement.is_some();
            if let Some(held) = current {
                superseded.push((held.id, held.owner));
            }
            if let Some(held) = replacement {
                superseded.push((held.id, held.owner));
            }
            tracked
        });
        let tracked = match transfer.await {
            Ok(tracked) => tracked,
            Err(error) => {
                tracing::error!(session_id = %session, %error, "credential release transfer failed");
                return;
            }
        };
        if tracked {
            // The periodic sweep owns provider I/O for locally transferred
            // credentials. Draining the global queue here would let ending one
            // session revoke another live session's just-replaced credential
            // before its full-sweep grace, and make termination wait for the
            // whole retry backlog.
            return;
        }

        let Some(issuer) = ({ self.issuer.read().await.clone() }) else {
            return;
        };

        /*
         * A restarted process has no `issued` entry. The key's deployment- and
         * session-scoped name is the durable ownership record, so ask every
         * owner whose token this process currently holds and take back every
         * match. More than one is possible after an older restart duplicated a
         * key, and release means none of them should survive.
         */
        let owner_tokens: Vec<String> = self
            .tokens
            .read()
            .await
            .values()
            .map(|tokens| tokens.access_token.clone())
            .collect();
        if owner_tokens.is_empty() {
            tracing::warn!(
                session_id = %session,
                "no owner token is held to find this ended session's credential; it remains valid"
            );
            return;
        }
        for owner_token in owner_tokens {
            let ids = match issuer.minted_for(session, &owner_token).await {
                Ok(ids) => ids,
                Err(error) => {
                    tracing::warn!(
                        session_id = %session,
                        error = %error,
                        "could not inspect an owner's keys for this ended session"
                    );
                    continue;
                }
            };
            for id in ids {
                /*
                 * An exact queue entry already owns this revoke. The read lock
                 * is a suffix of the global issued -> superseded -> reissues
                 * order and is dropped before provider I/O. If an entry races
                 * in after this check, reconcile it only after a confirmed
                 * successful DELETE; a non-2xx response must leave debt intact.
                 */
                if self.revoke_is_queued(&id).await {
                    continue;
                }
                // The same complete listing is the retry record for a failed
                // DELETE, so no separate in-memory queue is needed here.
                if issuer.revoke(&id, &owner_token).await {
                    self.superseded
                        .write()
                        .await
                        .retain(|(queued, _)| queued != &id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        extract::{Path, State},
        http::{header::AUTHORIZATION, HeaderMap, StatusCode},
        routing::{delete, get},
        Json, Router,
    };
    use layer2_core::CredentialSource as _;
    use tokio::sync::Mutex;

    use super::*;
    use crate::auth::{
        keys::{key_name, KeyIssuer},
        AuthMode, SubjectTokens,
    };

    #[derive(Clone)]
    struct Platform {
        keys: Vec<(String, String)>,
        deleted: Arc<Mutex<Vec<String>>>,
    }

    async fn list_keys(
        State(platform): State<Platform>,
        headers: HeaderMap,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        Json(serde_json::json!({
            "keys": platform
                .keys
                .iter()
                .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
                .collect::<Vec<_>>()
        }))
    }

    async fn delete_key(
        State(platform): State<Platform>,
        Path(id): Path<String>,
        headers: HeaderMap,
    ) -> StatusCode {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer owner-access")
        );
        platform.deleted.lock().await.push(id);
        StatusCode::NO_CONTENT
    }

    async fn platform(
        keys: Vec<(String, String)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let deleted = Arc::new(Mutex::new(Vec::new()));
        let state = Platform {
            keys,
            deleted: Arc::clone(&deleted),
        };
        let app = Router::new()
            .route("/api/v1/api-keys", get(list_keys))
            .route("/api/v1/api-keys/{id}", delete(delete_key))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock identity platform");
        let base = format!("http://{}", listener.local_addr().expect("mock address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock identity platform");
        });
        (base, deleted, server)
    }

    fn tokens() -> SubjectTokens {
        SubjectTokens {
            access_token: "owner-access".into(),
            refresh_token: None,
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(600),
        }
    }

    async fn auth(base: &str) -> Arc<AuthState> {
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
        );
        auth.use_issuer(Arc::new(KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "production",
        )))
        .await;
        auth
    }

    #[tokio::test]
    async fn a_restarted_process_finds_and_revokes_the_ended_sessions_key() {
        let session = SessionId("sesn_restarted".into());
        let keys = vec![
            ("key-for-session".into(), key_name("production", &session)),
            (
                "legacy-key-for-session".into(),
                "chrono-bot:production:sesn_restarted".into(),
            ),
            (
                "key-for-another-session".into(),
                key_name("production", &SessionId("sesn_other".into())),
            ),
            (
                "key-from-another-deployment".into(),
                key_name("staging", &session),
            ),
        ];
        let (base, deleted, server) = platform(keys).await;
        let auth = auth(&base).await;
        let owner = SubjectId("owner".into());
        auth.store_tokens(&owner, tokens()).await;
        assert!(
            auth.issued.read().await.is_empty(),
            "this process must never have minted the key it is releasing"
        );

        auth.release(&session).await;

        assert_eq!(
            deleted.lock().await.as_slice(),
            ["key-for-session", "legacy-key-for-session"]
        );
        server.abort();
    }

    #[tokio::test]
    async fn a_release_without_its_owners_token_stays_queued_until_the_token_returns() {
        let (base, deleted, server) = platform(Vec::new()).await;
        let auth = auth(&base).await;
        let session = SessionId("sesn_queued".into());
        let owner = SubjectId("owner".into());
        auth.issued.write().await.insert(
            session.clone(),
            Issued {
                id: "queued-key".into(),
                value: "session-value".into(),
                owner: owner.clone(),
                minted_at: std::time::Instant::now(),
            },
        );

        auth.release(&session).await;

        assert!(!auth.issued.read().await.contains_key(&session));
        assert_eq!(auth.superseded.read().await.len(), 1);
        assert_eq!(auth.take_back_superseded().await, 0);
        assert_eq!(
            auth.superseded.read().await.len(),
            1,
            "a sweep without the owner token must not forget the queued key"
        );

        auth.store_tokens(&owner, tokens()).await;
        assert_eq!(auth.take_back_superseded().await, 1);
        assert!(auth.superseded.read().await.is_empty());
        assert_eq!(deleted.lock().await.as_slice(), ["queued-key"]);
        server.abort();
    }
}
