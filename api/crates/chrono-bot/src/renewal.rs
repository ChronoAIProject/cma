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

/// How long a minted session credential is kept before it is replaced.
///
/// The platform does not give these keys an expiry. Twelve hours is a local
/// rotation cadence: it limits how long one value is used without pretending
/// the provider will invalidate it on that schedule.
const CREDENTIAL_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

/// One pass: renew what is close to expiry, then push it into live sessions.
async fn sweep_once(auth: &Arc<AuthState>, service: &Arc<SessionService>) {
    // Credentials queued on an earlier pass or release. Replaced credentials
    // wait so the sessions holding them have a full sweep to receive the value
    // that supersedes them; released credentials wait for an owner token when
    // release could not attempt the revoke itself.
    let taken = auth.take_back_superseded().await;
    if taken > 0 {
        tracing::info!(
            count = taken,
            "took back credentials that had been replaced"
        );
    }

    // Credentials this deployment minted that no session accounts for.
    //
    // A control plane that stopped between minting and releasing leaves one
    // behind, and nothing else records the pairing — the key's own name is
    // what says whose it was. Reclaiming needs the owner's token, which this
    // process may not hold after a restart, and there is no expiry behind it:
    // until that owner signs in again the key stays valid. This sweep is the
    // only thing that takes it back, so a pass it cannot make is a credential
    // still open, not a tidy-up postponed.
    let reclaimed = auth.reclaim_orphaned_credentials(service).await;
    if reclaimed > 0 {
        tracing::info!(
            count = reclaimed,
            "reclaimed credentials no session accounts for"
        );
    }

    // Sessions whose own credential is old enough to swap. Distinct from the
    // subject refresh below: that keeps this service able to mint, this keeps
    // the sessions able to act.
    for (session, owner) in auth.sessions_due_for_reissue(CREDENTIAL_LIFETIME).await {
        let services = service.session_services(&session).await;
        let Some(value) = auth.reissue(&session, &owner, &services).await else {
            tracing::warn!(session_id = %session, "could not mint a replacement credential");
            continue;
        };
        if service.push_session_credential(&session, value).await {
            tracing::info!(session_id = %session, "session credential replaced");
        } else {
            tracing::warn!(session_id = %session, "minted a replacement nothing could receive");
        }
    }

    let due = auth.subjects_needing_refresh(RENEW_MARGIN).await;
    if due.is_empty() {
        return;
    }
    tracing::debug!(subjects = due.len(), "renewing delegated credentials");

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
    let sessions_run_on_own_keys = auth.sessions_hold_own_credentials().await;
    for (subject, refresh_token) in due {
        match auth.refresh(&subject, &refresh_token).await {
            Ok(access_token) => {
                if sessions_run_on_own_keys {
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
