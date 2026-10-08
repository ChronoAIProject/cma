//! Per-request owner authority (Amendment R6 §R6.4). Every request checks
//! through REST's creator cache, so a demotion or a deactivation stops the
//! console within 60 s; plus one cache fill per session, refresh
//! classification, fail-closed unavailability, locking, expiry and the route
//! table walk.
use super::support::{csrf_for, csrf_token, metrics, sample, Harness};
use cmaeg::console::{
    authority::Denied,
    session::{Flash, OwnerSelection, SessionCell},
    table::{Access, ROUTES},
    OwnerScope,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

/// A record id for the route walk's channel routes.
const RECORD: &str = "00000000-0000-4000-8000-000000000001";
/// One second past REST's 60 s success TTL.
const PAST_CACHE: Duration = Duration::from_secs(61);

async fn signed_in(harness: &Harness, subject: &str) -> (String, Arc<SessionCell>) {
    let (sid, _) = harness.sign_in(subject, "/console").await;
    let cell = harness.console.sessions().find(&sid).unwrap();
    (sid, cell)
}

async fn page(harness: &Harness, sid: &str) -> String {
    harness.get_as("/console", sid).await.text().await.unwrap()
}

#[tokio::test]
async fn a_demoted_admin_loses_the_organization_within_60_seconds() {
    for removal in ["demoted", "deleted"] {
        let harness = Harness::enabled().await;
        let oauth = &harness.nyxid.oauth;
        oauth.grant("org-a", "Org A", "alice", "admin");
        // One session reads with a GET, the other writes with a POST.
        let (reader, reader_cell) = signed_in(&harness, "alice").await;
        let (writer, writer_cell) = signed_in(&harness, "alice").await;
        for sid in [&reader, &writer] {
            assert_eq!(harness.switch_owner(sid, "org-a").await.status(), 303);
        }
        let csrf = csrf_for(&page(&harness, &writer).await, "/console/owner");
        match removal {
            "demoted" => oauth.grant("org-a", "Org A", "alice", "member"),
            _ => oauth.remove_org("org-a"),
        }
        // REST's cache answers for up to 60 s, exactly as it does for REST.
        harness.advance(Duration::from_secs(59)).await;
        assert!(page(&harness, &reader)
            .await
            .contains("Channels of organization Org A."));
        harness.advance(Duration::from_secs(2)).await;
        // A GET is served as Personal, so an organization record reads 404.
        let shown = page(&harness, &reader).await;
        assert!(
            shown.contains("Channels of your personal account."),
            "{removal}: {shown}"
        );
        assert!(shown.contains("Your admin access to"), "{removal}: {shown}");
        let personal = harness
            .console
            .authority()
            .authorize(&reader_cell)
            .await
            .unwrap();
        assert_eq!(
            (personal.owner(), personal.scope()),
            ("alice", OwnerScope::Personal)
        );
        // A POST makes no change and goes home, where the flash explains it.
        let response = harness
            .post(
                "/console/owner",
                Some(&writer),
                None,
                &[("csrf", &csrf), ("owner", "org-a")],
            )
            .await;
        assert_eq!(response.status(), 303);
        assert_eq!(response.headers()["location"], "/console");
        for cell in [&reader_cell, &writer_cell] {
            assert_eq!(cell.meta().selected, OwnerSelection::Personal);
        }
        assert_eq!(
            writer_cell.meta().flash,
            Some(Flash::AdminEnded {
                organization: "org-a".into()
            })
        );
        assert_eq!(oauth.refresh_calls(), 0, "a refusal never forces a refresh");
        assert!(
            sample(
                &metrics(),
                "cmaeg_console_authorizations_total{outcome=\"downgraded\"}"
            ) >= 2.0
        );
        harness.close().await;
    }
}

#[tokio::test]
async fn a_deactivated_user_is_signed_out_within_60_seconds() {
    let harness = Harness::enabled().await;
    let (reader, reader_cell) = signed_in(&harness, "alice").await;
    let (writer, writer_cell) = signed_in(&harness, "alice").await;
    let csrf = csrf_for(&page(&harness, &writer).await, "/console/owner");
    harness
        .nyxid
        .oauth
        .deactivated
        .lock()
        .unwrap()
        .insert("alice".into());
    harness.advance(Duration::from_secs(59)).await;
    assert!(page(&harness, &reader).await.contains("Signed in as"));
    harness.advance(Duration::from_secs(2)).await;
    let response = harness.get_as("/console", &reader).await;
    assert_eq!(response.status(), 200);
    assert!(response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .starts_with("__Host-cmaeg-console=;"));
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("Sign in with NyxID"));
    // A POST in that state changes nothing and reports the ended session.
    let response = harness
        .post(
            "/console/owner",
            Some(&writer),
            None,
            &[("csrf", &csrf), ("owner", "personal")],
        )
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(
        response.headers()["location"],
        "/console/signed-out?reason=expired"
    );
    assert!(reader_cell.is_ended() && writer_cell.is_ended());
    assert!(harness.console.sessions().is_empty());
    assert_eq!(harness.nyxid.oauth.refresh_calls(), 0);
    harness.close().await;
}

#[tokio::test]
async fn an_outage_fails_closed_without_channel_data_and_keeps_the_session() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Secret Org Name", "carol", "admin");
    let (carol, carol_cell) = signed_in(&harness, "carol").await;
    harness.switch_owner(&carol, "org-a").await;
    let (dave, _) = signed_in(&harness, "dave").await;
    harness.advance(PAST_CACHE).await;
    oauth.down.store(true, Ordering::SeqCst);
    for sid in [&carol, &dave] {
        let response = harness.get_as("/console", sid).await;
        assert_eq!(response.status(), 503);
        let text = response.text().await.unwrap();
        assert!(text.contains("Console unavailable"));
        for fixture in ["Secret Org Name", "org-a", "arol", "dave"] {
            assert!(!text.contains(fixture), "{fixture}");
        }
    }
    assert_eq!(harness.console.sessions().len(), 2);
    // NyxID returns. REST caches the failure for 10 s, then service resumes
    // with the selection kept.
    oauth.down.store(false, Ordering::SeqCst);
    assert_eq!(harness.get_as("/console", &carol).await.status(), 503);
    harness.advance(Duration::from_secs(11)).await;
    assert!(page(&harness, &carol)
        .await
        .contains("Channels of organization Secret Org Name."));
    assert_eq!(
        carol_cell.meta().selected,
        OwnerSelection::Organization("org-a".into())
    );
    assert_eq!(harness.get_as("/console", &dave).await.status(), 200);
    assert!(
        sample(
            &metrics(),
            "cmaeg_console_authorizations_total{outcome=\"unavailable\"}"
        ) >= 2.0
    );
    harness.close().await;
}

#[tokio::test]
async fn a_session_makes_one_cache_fill_per_minute() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    let (sid, _) = signed_in(&harness, "alice").await;
    let identity = || oauth.users_me_calls.load(Ordering::SeqCst);
    // Two concurrent requests make one check.
    harness.advance(PAST_CACHE).await;
    let before = identity();
    let (first, second) = tokio::join!(
        harness.get_as("/console", &sid),
        harness.get_as("/console", &sid)
    );
    assert_eq!(
        (first.status().as_u16(), second.status().as_u16()),
        (200, 200)
    );
    assert_eq!(identity(), before + 1);
    // Ten requests within 60 s make one.
    harness.advance(PAST_CACHE).await;
    let before = identity();
    for _ in 0..10 {
        assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
        harness.advance(Duration::from_secs(5)).await;
    }
    assert_eq!(identity(), before + 1);
    // An organization view fills its own entry once as well.
    harness.switch_owner(&sid, "org-a").await;
    let authorizations = oauth.authorization_calls.load(Ordering::SeqCst);
    for _ in 0..10 {
        assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
        harness.advance(Duration::from_secs(5)).await;
    }
    assert_eq!(
        oauth.authorization_calls.load(Ordering::SeqCst),
        authorizations
    );
    harness.close().await;
}

#[tokio::test]
async fn refresh_failures_are_classified_on_every_request() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    let (sid, _) = signed_in(&harness, "alice").await;
    // No refresh while the token has more than 30 s left, so a broken token
    // endpoint goes unnoticed.
    harness.advance(Duration::from_secs(300)).await;
    oauth.token_status.store(503, Ordering::SeqCst);
    assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
    assert_eq!(oauth.refresh_calls(), 0);
    // Within 30 s of expiry it shows the 503 page and keeps the session.
    harness.advance(Duration::from_secs(570)).await;
    let response = harness.get_as("/console", &sid).await;
    assert_eq!(response.status(), 503);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("Console unavailable"));
    assert_eq!(harness.console.sessions().len(), 1);
    // NyxID recovers: the next refresh after the retry window serves again.
    oauth.token_status.store(200, Ordering::SeqCst);
    harness.advance(Duration::from_secs(11)).await;
    assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
    assert_eq!(oauth.refresh_calls(), 2);
    // invalid_grant ends the session once the rotated token is due.
    harness.advance(Duration::from_secs(870)).await;
    oauth.token_status.store(400, Ordering::SeqCst);
    let response = harness.get_as("/console", &sid).await;
    assert_eq!(response.status(), 200);
    assert!(response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .starts_with("__Host-cmaeg-console=;"));
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("Sign in with NyxID"));
    assert!(harness.console.sessions().is_empty());
    let rendered = metrics();
    for outcome in ["refresh_transient", "refreshed", "ended"] {
        let series = format!("cmaeg_console_authorizations_total{{outcome=\"{outcome}\"}}");
        assert!(sample(&rendered, &series) >= 1.0, "{outcome}");
    }
    harness.close().await;
}

#[tokio::test]
async fn a_hanging_check_never_blocks_other_sessions_and_one_session_refreshes_once() {
    let harness = Arc::new(Harness::enabled().await);
    let (stuck, _) = signed_in(&harness, "alice").await;
    let (bob, _) = signed_in(&harness, "bob").await;
    // Past the entries the sign-ins filled, so both checks reach NyxID.
    harness.advance(PAST_CACHE).await;
    harness
        .nyxid
        .oauth
        .hang
        .lock()
        .unwrap()
        .insert("alice".into());
    let hanging = tokio::spawn({
        let harness = harness.clone();
        async move { harness.get_as("/console", &stuck).await.status() }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    assert_eq!(harness.get_as("/console", &bob).await.status(), 200);
    assert!(started.elapsed() < Duration::from_secs(1));
    // NyxID's silence fails closed after REST's 3 s check timeout.
    assert_eq!(hanging.await.unwrap(), 503);
    harness.advance(Duration::from_secs(870) - PAST_CACHE).await;
    harness
        .nyxid
        .oauth
        .token_delay_ms
        .store(200, Ordering::SeqCst);
    let before = harness.nyxid.oauth.refresh_calls();
    let (first, second) = tokio::join!(
        harness.get_as("/console", &bob),
        harness.get_as("/console", &bob)
    );
    assert_eq!(
        (first.status().as_u16(), second.status().as_u16()),
        (200, 200)
    );
    assert_eq!(harness.nyxid.oauth.refresh_calls(), before + 1);
    harness.close_shared().await;
}

#[tokio::test]
async fn sessions_end_after_30_idle_minutes_or_8_hours() {
    let harness = Harness::enabled().await;
    let (idle, _) = signed_in(&harness, "alice").await;
    let (busy, _) = signed_in(&harness, "bob").await;
    let step = Duration::from_secs(29 * 60);
    harness.advance(step).await;
    assert_eq!(harness.get_as("/console", &busy).await.status(), 200);
    harness.advance(Duration::from_secs(60)).await;
    assert!(page(&harness, &idle).await.contains("Sign in with NyxID"));
    // Bob stays active, and is signed out only at the absolute 8 hours.
    assert!(page(&harness, &busy).await.contains("Signed in as"));
    let mut elapsed = step + Duration::from_secs(60);
    while elapsed + step < Duration::from_secs(8 * 3600) {
        harness.advance(step).await;
        elapsed += step;
        assert!(page(&harness, &busy).await.contains("Signed in as"));
    }
    harness.advance(step).await;
    assert!(page(&harness, &busy).await.contains("Sign in with NyxID"));
    assert!(harness.console.sessions().is_empty());
    let rendered = metrics();
    for reason in ["idle", "absolute"] {
        let series = format!("cmaeg_console_session_evictions_total{{reason=\"{reason}\"}}");
        assert_eq!(sample(&rendered, &series), 1.0, "{reason}");
    }
    harness.close().await;
}

#[tokio::test]
async fn logout_ends_the_session_while_nyxid_is_down() {
    let harness = Harness::enabled().await;
    let (sid, _) = signed_in(&harness, "alice").await;
    let csrf = csrf_for(&page(&harness, &sid).await, "/console/logout");
    harness.nyxid.oauth.down.store(true, Ordering::SeqCst);
    harness
        .nyxid
        .oauth
        .token_status
        .store(503, Ordering::SeqCst);
    let response = harness
        .post("/console/logout", Some(&sid), None, &[("csrf", &csrf)])
        .await;
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/console/signed-out");
    assert!(harness.console.sessions().is_empty());
    harness.close().await;
}

#[tokio::test]
async fn every_route_but_sign_in_plumbing_derives_authority() {
    let public: Vec<&str> = ROUTES
        .iter()
        .filter(|route| route.access == Access::Public)
        .map(|route| route.path)
        .collect();
    assert_eq!(
        public,
        [
            "/console/login",
            "/console/callback",
            "/console/signed-out",
            "/console/logout"
        ]
    );
    assert!(ROUTES
        .iter()
        .all(|route| matches!(route.access, Access::Public | Access::Authorized)));
    let harness = Harness::enabled().await;
    let (sid, _) = signed_in(&harness, "alice").await;
    // The forms' CSRF values, read while NyxID still answers.
    let home = page(&harness, &sid).await;
    harness.nyxid.oauth.down.store(true, Ordering::SeqCst);
    harness.advance(PAST_CACHE).await;
    let key = harness
        .console
        .sessions()
        .find(&sid)
        .unwrap()
        .meta()
        .csrf_key;
    for route in ROUTES.iter().filter(|route| route.access != Access::Public) {
        let path = route
            .path
            .replace("{record_id}", RECORD)
            .replace("{source_id}", "custom")
            .replace("{event_key}", "0123456789abcdef");
        let (anonymous, authorized) = match route.method {
            "GET" => (harness.get(&path).await, harness.get_as(&path, &sid).await),
            _ => {
                // Channel forms appear only on channel pages; mint theirs.
                let csrf = if route.path.contains("{record_id}") {
                    csrf_token(&key, route.path, RECORD)
                } else {
                    csrf_for(&home, route.path)
                };
                (
                    harness.post(&path, None, None, &[]).await,
                    harness
                        .post(&path, Some(&sid), None, &[("csrf", &csrf)])
                        .await,
                )
            }
        };
        match route.method {
            "GET" => assert_eq!(anonymous.status(), 200, "{}", route.path),
            _ => assert_eq!(
                anonymous.headers()["location"],
                "/console/signed-out?reason=expired"
            ),
        }
        // With NyxID down, a route that derives authority fails closed.
        assert_eq!(authorized.status(), 503, "{} {}", route.method, route.path);
    }
    // The authority API itself reports the outage and keeps the session.
    let cell = harness.console.sessions().find(&sid).unwrap();
    assert_eq!(
        harness.console.authority().authorize(&cell).await,
        Err(Denied::Unavailable)
    );
    harness.close().await;
}
