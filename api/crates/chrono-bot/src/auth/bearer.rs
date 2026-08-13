//! Establishing who is calling from a bearer token.
//!
//! The browser signs in and gets a cookie. Nothing else can: chrono-bot's
//! stated clients are other products' backends, and a server has no cookie jar,
//! no redirect to follow and no browser to be sent to a login page. Without
//! this the REST interface is reachable only by the one client it was not
//! primarily built for.
//!
//! The token is not decoded here, and deliberately so. Reading a token's claims
//! without checking who issued it accepts anything shaped like a token; the
//! provider is asked instead, which answers both "is this valid" and "whose is
//! it" in one call, and stops answering the moment the token is revoked.
//!
//! That call is cached, because it happens on every request and a round trip
//! per request would put the provider on the critical path of the terminal. The
//! cache is short and holds only what a request needs, so a revoked token stops
//! working within that window rather than at some later renewal.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::http::{header, HeaderMap};
use layer2_core::{Authorization, SubjectId};
use tokio::sync::RwLock;

use crate::auth::profile::Profile;

/// How long an answer about a token is reused.
///
/// Short enough that a withdrawn token stops working promptly, long enough that
/// a client making a burst of calls does not make the provider answer the same
/// question for each. This is the window in which a revoked token still works,
/// so it is stated as such rather than tuned for throughput.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// What the provider said about a token, and when.
struct Known {
    subject: SubjectId,
    authorization: Authorization,
    profile: Profile,
    at: Instant,
}

/// Answers about bearer tokens, remembered briefly.
#[derive(Default)]
pub struct BearerCache {
    known: RwLock<HashMap<String, Known>>,
}

impl BearerCache {
    /// What is known about this token, if it was asked about recently.
    pub async fn get(&self, token: &str) -> Option<(SubjectId, Authorization, Profile)> {
        let known = self.known.read().await;
        let entry = known.get(token)?;
        if entry.at.elapsed() > CACHE_TTL {
            return None;
        }
        Some((
            entry.subject.clone(),
            entry.authorization.clone(),
            entry.profile.clone(),
        ))
    }

    /// Remember what the provider said, and forget what has gone stale.
    ///
    /// Swept on write rather than on a timer: entries are only added by a
    /// request, so there is no moment where the map can grow without one, and a
    /// process nobody is calling should not be waking up to tidy an empty map.
    pub async fn remember(
        &self,
        token: &str,
        subject: SubjectId,
        authorization: Authorization,
        profile: Profile,
    ) {
        let mut known = self.known.write().await;
        known.retain(|_, e| e.at.elapsed() <= CACHE_TTL);
        known.insert(
            token.to_string(),
            Known {
                subject,
                authorization,
                profile,
                at: Instant::now(),
            },
        );
    }
}

/// The bearer token on a request, if it carries one.
///
/// The scheme is matched case-insensitively because RFC 7235 says it is, and a
/// client sending `bearer` in lower case is not making a mistake worth failing
/// their request over.
pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, value) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = value.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::Capability;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, value.parse().expect("valid header"));
        h
    }

    #[test]
    fn a_bearer_token_is_found() {
        assert_eq!(
            bearer_token(&headers("Bearer abc.def.ghi")).as_deref(),
            Some("abc.def.ghi")
        );
    }

    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        // RFC 7235 says the scheme is case-insensitive, and a client that sends
        // it in lower case is not making a mistake worth a 401 over.
        assert_eq!(bearer_token(&headers("bearer abc")).as_deref(), Some("abc"));
    }

    #[test]
    fn another_scheme_is_not_a_bearer_token() {
        // Reading a basic credential as a token would send someone's password
        // to the provider as though it were one.
        assert!(bearer_token(&headers("Basic dXNlcjpwYXNz")).is_none());
    }

    #[test]
    fn an_empty_token_is_no_token() {
        // Otherwise an empty string is asked about, which the provider answers
        // for with a 401 that reads as "your token is bad" rather than "you
        // sent none".
        assert!(bearer_token(&headers("Bearer   ")).is_none());
    }

    #[test]
    fn a_request_with_no_authorization_carries_no_token() {
        assert!(bearer_token(&HeaderMap::new()).is_none());
    }

    #[tokio::test]
    async fn a_remembered_token_is_answered_without_asking_again() {
        // The provider is otherwise on the critical path of every request,
        // including the ones driving a terminal.
        let cache = BearerCache::default();
        let subject = SubjectId("subject-a".into());
        cache
            .remember(
                "tok",
                subject.clone(),
                Authorization::from_capabilities([Capability::CreateSession]),
                Profile::default(),
            )
            .await;

        let (found, authorization, _) = cache.get("tok").await.expect("remembered");
        assert_eq!(found, subject);
        assert!(authorization.allows(Capability::CreateSession));
    }

    #[tokio::test]
    async fn an_unknown_token_is_not_answered_for() {
        let cache = BearerCache::default();
        assert!(cache.get("never-seen").await.is_none());
    }

    #[tokio::test]
    async fn one_token_does_not_answer_for_another() {
        // The obvious catastrophe: a cache keyed loosely would hand one
        // caller's identity to another.
        let cache = BearerCache::default();
        cache
            .remember(
                "tok-a",
                SubjectId("subject-a".into()),
                Authorization::from_capabilities([]),
                Profile::default(),
            )
            .await;
        assert!(cache.get("tok-b").await.is_none());
    }
}
