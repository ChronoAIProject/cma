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

use crate::auth::AuthState;

/// How often to look for credentials approaching expiry.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// How far ahead of expiry to renew.
///
/// Generous on purpose: renewing early costs one request, while renewing late
/// fails a request the user is waiting on.
const RENEW_MARGIN: Duration = Duration::from_secs(300);

/// Run the renewal loop until the process ends.
pub fn spawn(auth: Arc<AuthState>, service: Arc<SessionService>) {
    if !auth.requires_credential() {
        // Nothing to renew when identity is not coming from a provider.
        tracing::info!("credential renewal not started: no identity provider configured");
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            ticker.tick().await;
            sweep_once(&auth, &service).await;
        }
    });
    tracing::info!(
        sweep_seconds = SWEEP_INTERVAL.as_secs(),
        margin_seconds = RENEW_MARGIN.as_secs(),
        "credential renewal started"
    );
}

/// One pass: renew what is close to expiry, then push it into live sessions.
async fn sweep_once(auth: &Arc<AuthState>, service: &Arc<SessionService>) {
    let due = auth.subjects_needing_refresh(RENEW_MARGIN).await;
    if due.is_empty() {
        return;
    }
    tracing::debug!(subjects = due.len(), "renewing delegated credentials");

    for (subject, refresh_token) in due {
        match auth.refresh(&subject, &refresh_token).await {
            Ok(access_token) => {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthMode, SubjectTokens};
    use layer2_core::SubjectId;

    fn tokens(expires_in: Duration, refresh: Option<&str>) -> SubjectTokens {
        SubjectTokens {
            access_token: "access".into(),
            refresh_token: refresh.map(|r| r.to_string()),
            expires_at: std::time::Instant::now() + expires_in,
        }
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
            .await;
        auth.store_tokens(&stranded, tokens(Duration::from_secs(1), None))
            .await;

        let due = auth.subjects_needing_refresh(RENEW_MARGIN).await;
        let subjects: Vec<_> = due.iter().map(|(s, _)| s.as_str().to_string()).collect();
        assert!(subjects.contains(&"has-refresh".to_string()));
        assert!(
            !subjects.contains(&"no-refresh".to_string()),
            "a subject with no refresh token cannot be renewed and must not be retried"
        );
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
        assert_eq!(auth.access_token(&s).await, None);
        auth.store_tokens(&s, tokens(Duration::from_secs(600), Some("r")))
            .await;
        assert_eq!(auth.access_token(&s).await.as_deref(), Some("access"));
    }

    #[tokio::test]
    async fn renewal_does_not_start_without_a_provider() {
        // Trusted mode has no tokens to renew; starting a loop would be noise.
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local".into(),
            },
            None,
        );
        assert!(!auth.requires_credential());
    }
}
