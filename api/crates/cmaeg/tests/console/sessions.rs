//! In-memory sessions: single-flight refresh with rotation, refresh
//! classification, caps, expiry and best-effort revocation.
use super::support::{config, metrics, sample, Harness};
use cmaeg::{
    console::session::{EndReason, Refresh, SessionCell},
    ConsoleConfig,
};
use cmaeg_core::channel::Secret;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

/// When a fresh 900 s access token is first refreshed: 30 s before expiry.
const DUE: Duration = Duration::from_secs(870);

async fn harness_with(config: ConsoleConfig) -> Harness {
    let harness = Harness::start(config).await;
    harness.console.discover_once().await.unwrap();
    harness
}

fn create(harness: &Harness, subject: &str) -> (Secret, Arc<SessionCell>) {
    harness
        .console
        .sessions()
        .create(subject.into(), harness.tokens(subject))
        .unwrap()
}

async fn refresh(harness: &Harness, cell: &Arc<SessionCell>) -> Refresh {
    let mut auth = cell.lock_auth().await;
    harness
        .console
        .sessions()
        .refresh(cell, &mut auth, false)
        .await
}

async fn revoked_eventually(harness: &Harness, count: usize) -> Vec<String> {
    for _ in 0..200 {
        let revoked = harness.nyxid.oauth.revoked.lock().unwrap().clone();
        if revoked.len() >= count {
            return revoked;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected {count} revocations");
}

#[tokio::test]
async fn concurrent_requests_make_one_refresh_and_rotate_the_token() {
    let harness = harness_with(config()).await;
    let (_, cell) = create(&harness, "alice");
    let original = cell.lock_auth().await.access().clone();
    harness.clock.advance(DUE);
    harness
        .nyxid
        .oauth
        .token_delay_ms
        .store(200, Ordering::SeqCst);
    let (first, second) = tokio::join!(refresh(&harness, &cell), refresh(&harness, &cell));
    let mut outcomes = [first, second];
    outcomes.sort_by_key(|outcome| format!("{outcome:?}"));
    assert_eq!(outcomes, [Refresh::NotDue, Refresh::Rotated]);
    assert_eq!(harness.nyxid.oauth.refresh_calls(), 1);
    assert_ne!(cell.lock_auth().await.access(), &original);
    // The rotated refresh token is the one used next; the old one is gone.
    harness.clock.advance(DUE);
    assert_eq!(refresh(&harness, &cell).await, Refresh::Rotated);
    assert_eq!(harness.nyxid.oauth.refresh_calls(), 2);
    harness.close().await;
}

#[tokio::test]
async fn no_refresh_happens_until_the_token_expires_within_30_seconds() {
    let harness = harness_with(config()).await;
    let (_, cell) = create(&harness, "alice");
    harness.clock.advance(DUE - Duration::from_secs(1));
    assert_eq!(refresh(&harness, &cell).await, Refresh::NotDue);
    assert_eq!(harness.nyxid.oauth.refresh_calls(), 0);
    harness.clock.advance(Duration::from_secs(1));
    assert_eq!(refresh(&harness, &cell).await, Refresh::Rotated);
    assert_eq!(harness.nyxid.oauth.refresh_calls(), 1);
    harness.close().await;
}

#[tokio::test]
async fn invalid_grant_or_401_ends_the_session_and_revokes() {
    for (status, error) in [(400, "invalid_grant"), (401, "invalid_client")] {
        let harness = harness_with(config()).await;
        let tokens = harness.tokens("alice");
        let original = tokens.refresh.as_str().to_owned();
        let (sid, cell) = harness
            .console
            .sessions()
            .create("alice".into(), tokens)
            .unwrap();
        harness.clock.advance(DUE);
        let oauth = &harness.nyxid.oauth;
        oauth.token_status.store(status, Ordering::SeqCst);
        *oauth.token_error.lock().unwrap() = error.into();
        assert_eq!(refresh(&harness, &cell).await, Refresh::Ended);
        assert!(cell.is_ended());
        assert!(harness.console.sessions().find(sid.as_str()).is_none());
        let revoked = revoked_eventually(&harness, 1).await;
        assert_eq!(revoked, [original]);
        harness.close().await;
    }
}

#[tokio::test]
async fn transient_refresh_failures_keep_the_session_and_retry_after_ten_seconds() {
    for failure in ["503", "429", "malformed"] {
        let harness = harness_with(config()).await;
        let (sid, cell) = create(&harness, "alice");
        harness.clock.advance(DUE);
        let oauth = &harness.nyxid.oauth;
        match failure {
            "malformed" => oauth.token_malformed.store(true, Ordering::SeqCst),
            status => oauth
                .token_status
                .store(status.parse().unwrap(), Ordering::SeqCst),
        }
        assert_eq!(
            refresh(&harness, &cell).await,
            Refresh::Transient { usable: false },
            "{failure}"
        );
        assert!(harness.console.sessions().find(sid.as_str()).is_some());
        // No new call inside the 10 s retry window.
        harness.clock.advance(Duration::from_secs(9));
        assert_eq!(
            refresh(&harness, &cell).await,
            Refresh::Transient { usable: false }
        );
        assert_eq!(oauth.refresh_calls(), 1, "{failure}");
        oauth.token_status.store(200, Ordering::SeqCst);
        oauth.token_malformed.store(false, Ordering::SeqCst);
        harness.clock.advance(Duration::from_secs(1));
        assert_eq!(refresh(&harness, &cell).await, Refresh::Rotated);
        harness.close().await;
    }
}

#[tokio::test]
async fn a_timeout_keeps_the_session_and_a_nearly_expired_token_is_not_served() {
    let harness = harness_with(config()).await;
    let (sid, cell) = create(&harness, "alice");
    harness.clock.advance(Duration::from_secs(880));
    harness
        .nyxid
        .oauth
        .token_delay_ms
        .store(6_000, Ordering::SeqCst);
    assert_eq!(
        refresh(&harness, &cell).await,
        Refresh::Transient { usable: false }
    );
    assert!(harness.console.sessions().find(sid.as_str()).is_some());
    harness.close().await;
}

#[tokio::test]
async fn a_hanging_refresh_never_blocks_other_sessions() {
    let harness = Arc::new(harness_with(config()).await);
    let (_, stuck) = create(&harness, "alice");
    let (bob_sid, _) = create(&harness, "bob");
    harness.clock.advance(DUE);
    harness
        .nyxid
        .oauth
        .token_delay_ms
        .store(2_000, Ordering::SeqCst);
    let hanging = tokio::spawn({
        let harness = harness.clone();
        async move { refresh(&harness, &stuck).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    let bob = harness.console.sessions().find(bob_sid.as_str()).unwrap();
    drop(bob.lock_auth().await);
    create(&harness, "carol");
    harness.console.sessions().sweep();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(!hanging.is_finished());
    assert_eq!(hanging.await.unwrap(), Refresh::Rotated);
}

#[tokio::test]
async fn subject_and_global_caps_evict_and_revoke() {
    let capped = ConsoleConfig {
        sessions_per_subject: 2,
        sessions_max: 3,
        ..config()
    };
    let harness = harness_with(capped).await;
    let (first, first_cell) = create(&harness, "alice");
    harness.clock.advance(Duration::from_secs(1));
    let (second, _) = create(&harness, "alice");
    harness.clock.advance(Duration::from_secs(1));
    let (bob, _) = create(&harness, "bob");
    harness.clock.advance(Duration::from_secs(1));
    let (third, _) = create(&harness, "alice");
    // Alice's oldest went; bob's session is untouched by her loop.
    let sessions = harness.console.sessions();
    assert!(sessions.find(first.as_str()).is_none());
    assert!(first_cell.is_ended());
    assert!(sessions.find(bob.as_str()).is_some());
    assert!(sessions.find(third.as_str()).is_some());
    assert_eq!(sessions.len(), 3);
    harness.clock.advance(Duration::from_secs(1));
    // Seeing `second` makes `third` the least recently seen.
    assert!(sessions.find(second.as_str()).is_some());
    harness.clock.advance(Duration::from_secs(1));
    assert!(sessions.find(bob.as_str()).is_some());
    create(&harness, "carol");
    assert!(sessions.find(third.as_str()).is_none());
    assert_eq!(sessions.len(), 3);
    let rendered = metrics();
    assert_eq!(
        sample(
            &rendered,
            "cmaeg_console_session_evictions_total{reason=\"subject_cap\"}"
        ),
        1.0
    );
    assert_eq!(
        sample(
            &rendered,
            "cmaeg_console_session_evictions_total{reason=\"global_cap\"}"
        ),
        1.0
    );
    assert_eq!(sample(&rendered, "cmaeg_console_sessions"), 3.0);
    assert_eq!(revoked_eventually(&harness, 2).await.len(), 2);
    harness.close().await;
}

#[tokio::test]
async fn idle_and_absolute_expiry_end_sessions_and_the_sweep_revokes_them() {
    let short = ConsoleConfig {
        session_idle: Duration::from_secs(60),
        session_ttl: Duration::from_secs(600),
        ..config()
    };
    let harness = harness_with(short).await;
    let sessions = harness.console.sessions();
    let (idle, _) = create(&harness, "alice");
    harness.clock.advance(Duration::from_secs(59));
    assert!(sessions.find(idle.as_str()).is_some());
    harness.clock.advance(Duration::from_secs(60));
    assert!(sessions.find(idle.as_str()).is_none());
    let (busy, _) = create(&harness, "bob");
    for _ in 0..11 {
        harness.clock.advance(Duration::from_secs(50));
        assert!(sessions.find(busy.as_str()).is_some());
    }
    harness.clock.advance(Duration::from_secs(50));
    assert!(sessions.find(busy.as_str()).is_none());
    let (abandoned, abandoned_cell) = create(&harness, "carol");
    harness.clock.advance(Duration::from_secs(61));
    sessions.sweep();
    assert!(abandoned_cell.is_ended());
    assert!(sessions.find(abandoned.as_str()).is_none());
    assert!(sessions.is_empty());
    assert_eq!(revoked_eventually(&harness, 3).await.len(), 3);
    let rendered = metrics();
    assert_eq!(
        sample(
            &rendered,
            "cmaeg_console_session_evictions_total{reason=\"idle\"}"
        ),
        2.0
    );
    assert_eq!(
        sample(
            &rendered,
            "cmaeg_console_session_evictions_total{reason=\"absolute\"}"
        ),
        1.0
    );
    harness.close().await;
}

#[tokio::test]
async fn revocations_beyond_four_in_flight_are_dropped_and_counted() {
    let harness = harness_with(config()).await;
    harness
        .nyxid
        .oauth
        .revoke_delay_ms
        .store(500, Ordering::SeqCst);
    let cells: Vec<_> = (0..6)
        .map(|index| create(&harness, &format!("user-{index}")).1)
        .collect();
    for cell in &cells {
        harness.console.sessions().end(cell, EndReason::Logout);
    }
    assert_eq!(revoked_eventually(&harness, 4).await.len(), 4);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rendered = metrics();
    assert_eq!(
        sample(
            &rendered,
            "cmaeg_console_revocations_total{outcome=\"dropped\"}"
        ),
        2.0
    );
    assert_eq!(
        sample(&rendered, "cmaeg_console_revocations_total{outcome=\"ok\"}"),
        4.0
    );
    assert_eq!(harness.nyxid.oauth.revoked.lock().unwrap().len(), 4);
    harness.close().await;
}

#[tokio::test]
async fn a_session_id_is_never_stored_and_malformed_cookies_find_nothing() {
    let harness = harness_with(config()).await;
    let (sid, _) = create(&harness, "alice");
    assert_eq!(sid.as_str().len(), 43);
    let sessions = harness.console.sessions();
    for cookie in [
        "",
        "short",
        &"A".repeat(44),
        &format!("{}!", &sid.as_str()[..42]),
    ] {
        assert!(sessions.find(cookie).is_none(), "{cookie}");
    }
    assert!(sessions.find(sid.as_str()).is_some());
    harness.close().await;
}
