//! Per-request owner authority (Amendment R3): demotion within 60 s, stale
//! tokens, refresh classification, locking, the REST cache, fail-closed
//! unavailability, the permit bounds, and the route table walk.
use super::support::{metrics, sample, Harness};
use cmaeg::console::{
    authority::{Denied, Need},
    session::{Flash, OwnerSelection, SessionCell},
    table::{Access, ROUTES},
    OwnerScope,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

async fn signed_in(harness: &Harness, subject: &str) -> (String, Arc<SessionCell>) {
    let (sid, _) = harness.sign_in(subject, "/console").await;
    let cell = harness.console.sessions().find(&sid).unwrap();
    (sid, cell)
}

fn select(cell: &SessionCell, organization: &str) {
    cell.meta().selected = OwnerSelection::Organization(organization.into());
}

async fn read(harness: &Harness, cell: &Arc<SessionCell>) -> Result<OwnerScope, Denied> {
    harness
        .console
        .authority()
        .authorize(cell, Need::Read)
        .await
        .map(|authorized| authorized.scope())
}

#[tokio::test]
async fn a_demoted_admin_keeps_reads_for_60_seconds_then_drops_to_personal() {
    for removal in ["demoted", "deleted"] {
        let harness = Harness::enabled().await;
        let oauth = &harness.nyxid.oauth;
        oauth.grant("org-a", "Org A", "alice", "admin");
        let (_, cell) = signed_in(&harness, "alice").await;
        select(&cell, "org-a");
        assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Organization));
        match removal {
            "demoted" => oauth.grant("org-a", "Org A", "alice", "member"),
            _ => oauth.remove_org("org-a"),
        }
        harness.clock.advance(Duration::from_secs(59));
        assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Organization));
        harness.clock.advance(Duration::from_secs(2));
        let refreshes = oauth.refresh_calls();
        assert_eq!(
            read(&harness, &cell).await,
            Ok(OwnerScope::Personal),
            "{removal}"
        );
        // A refresh came first, so a stale token was ruled out.
        assert_eq!(oauth.refresh_calls(), refreshes + 1);
        {
            let meta = cell.meta();
            assert_eq!(meta.selected, OwnerSelection::Personal);
            assert_eq!(
                meta.flash,
                Some(Flash::AdminEnded {
                    organization: "org-a".into()
                })
            );
        }
        // A mutation right after the demotion is refused outright.
        select(&cell, "org-a");
        assert_eq!(
            harness
                .console
                .authority()
                .authorize(&cell, Need::Fresh)
                .await
                .err(),
            Some(Denied::Downgraded)
        );
        assert!(
            sample(
                &metrics(),
                "cmaeg_console_authorizations_total{outcome=\"downgraded\"}"
            ) >= 1.0
        );
        harness.close().await;
    }
}

#[tokio::test]
async fn a_stale_token_refreshes_and_keeps_the_organization_without_a_flash() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Org A", "alice", "admin");
    let (_, cell) = signed_in(&harness, "alice").await;
    select(&cell, "org-a");
    assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Organization));
    harness.clock.advance(Duration::from_secs(61));
    oauth.expire_access("alice");
    assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Organization));
    assert_eq!(oauth.refresh_calls(), 1);
    assert_eq!(cell.meta().flash, None);
    harness.close().await;
}

#[tokio::test]
async fn a_deactivated_user_is_signed_out_after_one_forced_refresh() {
    let harness = Harness::enabled().await;
    let (sid, cell) = signed_in(&harness, "alice").await;
    assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Personal));
    harness
        .nyxid
        .oauth
        .deactivated
        .lock()
        .unwrap()
        .insert("alice".into());
    harness.clock.advance(Duration::from_secs(59));
    assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Personal));
    harness.clock.advance(Duration::from_secs(2));
    assert_eq!(read(&harness, &cell).await, Err(Denied::SignedOut));
    assert_eq!(harness.nyxid.oauth.refresh_calls(), 1);
    assert!(cell.is_ended());
    let page = harness.get_as("/console", &sid).await;
    assert_eq!(page.status(), 200);
    assert!(page.text().await.unwrap().contains("Sign in with NyxID"));
    harness.close().await;
}

#[tokio::test]
async fn refresh_failures_are_classified_on_every_request() {
    let harness = Harness::enabled().await;
    let oauth = &harness.nyxid.oauth;
    let (sid, _) = signed_in(&harness, "alice").await;
    // A token-endpoint 503 with a still-valid access token keeps serving.
    harness.clock.advance(Duration::from_secs(300));
    oauth.token_status.store(503, Ordering::SeqCst);
    assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
    // Within 30 s of expiry it shows the 503 page and keeps the session.
    harness.clock.advance(Duration::from_secs(580));
    let page = harness.get_as("/console", &sid).await;
    assert_eq!(page.status(), 503);
    assert!(page.text().await.unwrap().contains("Console unavailable"));
    assert_eq!(harness.console.sessions().len(), 1);
    // NyxID recovers: the next refresh after the retry window serves again.
    oauth.token_status.store(200, Ordering::SeqCst);
    harness.clock.advance(Duration::from_secs(11));
    assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
    // invalid_grant ends the session.
    harness.clock.advance(Duration::from_secs(300));
    oauth.token_status.store(400, Ordering::SeqCst);
    let page = harness.get_as("/console", &sid).await;
    assert_eq!(page.status(), 200);
    assert!(page.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .starts_with("__Host-cmaeg-console=;"));
    assert!(page.text().await.unwrap().contains("Sign in with NyxID"));
    assert!(harness.console.sessions().is_empty());
    let rendered = metrics();
    assert!(
        sample(
            &rendered,
            "cmaeg_console_authorizations_total{outcome=\"refresh_transient\"}"
        ) >= 1.0
    );
    assert!(
        sample(
            &rendered,
            "cmaeg_console_authorizations_total{outcome=\"ended\"}"
        ) >= 1.0
    );
    harness.close().await;
}

#[tokio::test]
async fn a_hanging_check_never_blocks_other_sessions_and_one_session_refreshes_once() {
    let harness = Arc::new(Harness::enabled().await);
    let (stuck, _) = signed_in(&harness, "alice").await;
    let (bob, _) = signed_in(&harness, "bob").await;
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
    // NyxID's silence fails closed after the 3 s check timeout.
    assert_eq!(hanging.await.unwrap(), 503);
    harness.clock.advance(Duration::from_secs(300));
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
}

#[tokio::test]
async fn ten_minutes_of_page_refreshes_make_few_checks_and_leave_the_rest_cache_alone() {
    let harness = Harness::enabled().await;
    let (sid, _) = signed_in(&harness, "alice").await;
    let oauth = &harness.nyxid.oauth;
    let checks_after_sign_in = oauth.users_me_calls.load(Ordering::SeqCst);
    for _ in 0..60 {
        harness.clock.advance(Duration::from_secs(10));
        assert_eq!(harness.get_as("/console", &sid).await.status(), 200);
    }
    let read_checks = oauth.users_me_calls.load(Ordering::SeqCst) - checks_after_sign_in;
    assert!(read_checks <= 20, "{read_checks} read checks");
    let access = oauth.access_tokens_of("alice");
    let before = oauth.users_me_calls.load(Ordering::SeqCst);
    harness
        .creator_auth
        .authenticate_token(access.last().unwrap(), None)
        .await
        .unwrap();
    assert_eq!(oauth.users_me_calls.load(Ordering::SeqCst), before + 1);
    harness.close().await;
}

#[tokio::test]
async fn an_outage_fails_closed_and_permits_bound_checks() {
    let harness = Arc::new(Harness::enabled().await);
    let oauth = &harness.nyxid.oauth;
    oauth.grant("org-a", "Secret Org Name", "carol", "admin");
    let (carol, cell) = signed_in(&harness, "carol").await;
    select(&cell, "org-a");
    oauth.down.store(true, Ordering::SeqCst);
    let page = harness.get_as("/console", &carol).await;
    assert_eq!(page.status(), 503);
    let text = page.text().await.unwrap();
    assert!(
        !text.contains("Secret Org Name") && !text.contains("org-a") && !text.contains("carol")
    );
    assert_eq!(harness.console.sessions().len(), 1);
    // Recovery: after the 10 s unavailable verdict, service resumes.
    oauth.down.store(false, Ordering::SeqCst);
    assert_eq!(harness.get_as("/console", &carol).await.status(), 503);
    harness.clock.advance(Duration::from_secs(11));
    assert_eq!(harness.get_as("/console", &carol).await.status(), 200);

    // Eight hanging subjects hold every permit; a ninth waits 2 s, then 503.
    let mut hanging = Vec::new();
    for index in 0..8 {
        let subject = format!("hang-{index}");
        let (sid, _) = signed_in(&harness, &subject).await;
        oauth.hang.lock().unwrap().insert(subject);
        let harness = harness.clone();
        hanging.push(tokio::spawn(async move {
            harness.get_as("/console", &sid).await.status()
        }));
    }
    let (ninth, _) = signed_in(&harness, "ninth").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    assert_eq!(harness.get_as("/console", &ninth).await.status(), 503);
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(1_900) && waited < Duration::from_millis(2_900),
        "{waited:?}"
    );
    for task in hanging {
        assert_eq!(task.await.unwrap(), 503);
    }
    harness.close_shared().await;
}

#[tokio::test]
async fn one_subject_never_holds_more_than_one_permit() {
    let harness = Arc::new(Harness::enabled().await);
    let oauth = &harness.nyxid.oauth;
    let (first, _) = signed_in(&harness, "alice").await;
    let (second, _) = signed_in(&harness, "alice").await;
    let (bob, _) = signed_in(&harness, "bob").await;
    oauth.hang.lock().unwrap().insert("alice".into());
    let calls = oauth.users_me_calls.load(Ordering::SeqCst);
    let hanging = tokio::spawn({
        let harness = harness.clone();
        async move { harness.get_as("/console", &first).await.status() }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    assert_eq!(harness.get_as("/console", &second).await.status(), 503);
    assert!(started.elapsed() >= Duration::from_millis(1_900));
    // Alice's second session never reached NyxID; bob is served meanwhile.
    assert_eq!(oauth.users_me_calls.load(Ordering::SeqCst), calls + 1);
    assert_eq!(harness.get_as("/console", &bob).await.status(), 200);
    assert_eq!(hanging.await.unwrap(), 503);
    harness.close_shared().await;
}

#[tokio::test]
async fn a_mutation_verifies_afresh_despite_a_fresh_read_verdict() {
    let harness = Harness::enabled().await;
    let (_, cell) = signed_in(&harness, "alice").await;
    assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Personal));
    let calls = harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst);
    assert_eq!(read(&harness, &cell).await, Ok(OwnerScope::Personal));
    assert_eq!(
        harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst),
        calls
    );
    let fresh = harness
        .console
        .authority()
        .authorize(&cell, Need::Fresh)
        .await
        .unwrap();
    assert_eq!(fresh.owner(), "alice");
    assert_eq!(fresh.creator().subject, "alice");
    assert_eq!(
        harness.nyxid.oauth.users_me_calls.load(Ordering::SeqCst),
        calls + 1
    );
    harness.close().await;
}

#[tokio::test]
async fn logout_ends_the_session_while_nyxid_is_down() {
    let harness = Harness::enabled().await;
    let (sid, _) = signed_in(&harness, "alice").await;
    let page = harness.get_as("/console", &sid).await.text().await.unwrap();
    let csrf = super::support::csrf_for(&page, "/console/logout");
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
    let harness = Harness::enabled().await;
    let (sid, _) = signed_in(&harness, "alice").await;
    // The forms' CSRF values, read while NyxID still answers.
    let home = harness.get_as("/console", &sid).await.text().await.unwrap();
    harness.nyxid.oauth.down.store(true, Ordering::SeqCst);
    harness.clock.advance(std::time::Duration::from_secs(61));
    for route in ROUTES.iter().filter(|route| route.access != Access::Public) {
        let (anonymous, authorized) = match route.method {
            "GET" => (
                harness.get(route.path).await,
                harness.get_as(route.path, &sid).await,
            ),
            _ => {
                let csrf = super::support::csrf_for(&home, route.path);
                (
                    harness.post(route.path, None, None, &[]).await,
                    harness
                        .post(route.path, Some(&sid), None, &[("csrf", &csrf)])
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
    harness.close().await;
}
