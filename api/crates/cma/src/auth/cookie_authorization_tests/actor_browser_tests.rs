use super::*;
use crate::auth::actor_links::{ActorLinkOrigin, ActorWakeupState};
use layer2_core::sandbox::agent_io::ExternalEventContext;

#[path = "actor_delegated_tests.rs"]
mod delegated_tests;

fn origin() -> ActorLinkOrigin {
    ActorLinkOrigin {
        binding_generation: 7,
        provider_conversation_id: "provider-selected-sender-a".into(),
        agent_id: layer2_core::AgentId("agent-a".into()),
        worktree_id: layer2_core::WorktreeId("worktree-a".into()),
        source_event: ExternalEventContext {
            source_id: "channel-a".into(),
            conversation_id: "chat-a".into(),
            provider_conversation_id: Some("provider-selected-sender-a".into()),
            event_id: "event-a".into(),
            reference: "event_ref_fixture".into(),
            payload: json!({"actor":{"id":"sender-a"},"activity":{"text":"Create a paper bot"}}),
        },
        reply_target_ref: "reply_target_fixture".into(),
        reply_expires_at: epoch() + 86400,
    }
}
fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
fn cookie(response: &axum::response::Response, prefix: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(prefix))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}
async fn call(f: &Fixture, request: Request<Body>) -> axum::response::Response {
    crate::auth::router(f.auth.clone())
        .oneshot(request)
        .await
        .unwrap()
}
async fn start(f: &Fixture) -> (String, String, String) {
    let link = f
        .auth
        .ensure_actor_link_with_origin(&actor(), origin())
        .await
        .unwrap();
    let response = call(
        f,
        Request::get(format!("/auth/bot-father/link/{}", link.link_id))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let url = reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    (link.link_id, state, cookie(&response, "cma_login_"))
}
async fn finish(f: &Fixture, state: &str, binding: &str) -> axum::response::Response {
    call(
        f,
        Request::get(format!("/auth/callback?state={state}&code=fixture-code"))
            .header(header::COOKIE, binding)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}
async fn ready(f: &Fixture) -> (String, String) {
    let (link, state, binding) = start(f).await;
    let response = finish(f, &state, &binding).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    (link, cookie(&response, &format!("{AUTH_COOKIE}=")))
}
async fn consent(
    f: &Fixture,
    link: &str,
    browser: &str,
    origin: &str,
    csrf: &str,
    subject: &str,
    revision: u64,
) -> axum::response::Response {
    call(
        f,
        Request::post(format!("/auth/bot-father/link/{link}/consent"))
            .header(header::COOKIE, browser)
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "csrf_token={csrf}&subject={subject}&review_revision={revision}"
            )))
            .unwrap(),
    )
    .await
}
async fn approve(f: &Fixture, link: &str, browser: &str) -> axum::response::Response {
    let row = f.store.actor_link_by_id(link).await.unwrap().unwrap();
    let evidence = row.browser_consent.unwrap();
    consent(
        f,
        link,
        browser,
        "https://cma.example.test",
        &evidence.csrf_token,
        "owner",
        evidence.review_revision,
    )
    .await
}

#[tokio::test]
async fn actor_browser_consent_requires_fresh_pkce_attempt_and_exact_browser() {
    let f = fixture().await;
    let unrelated = f
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    let (link, state, binding) = start(&f).await;
    let review = |cookie: String| {
        Request::get(format!("/auth/bot-father/link/{link}/complete"))
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        call(&f, review(format!("{AUTH_COOKIE}={unrelated}")))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        finish(
            &f,
            &state,
            &format!("{}=wrong", binding.split_once('=').unwrap().0)
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let response = finish(&f, &state, &binding).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let browser = cookie(&response, &format!("{AUTH_COOKIE}="));
    assert_eq!(
        finish(&f, &state, &binding).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&f, review(format!("{AUTH_COOKIE}={unrelated}")))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let response = call(&f, review(browser.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 65536)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("sender-a") && html.contains("chat-a") && html.contains("NyxID account"));
    assert!(!html.contains("<pre>link_") && !html.contains("rotated-fixture-token"));
    let row = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
    assert!(!row.active && row.wakeup.is_none());
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    assert!(f.auth.current_actor_grant(&link, &actor()).await.is_ok());
}

#[tokio::test]
async fn actor_browser_wrong_csrf_origin_subject_link_and_revision_do_not_consume_consent() {
    let f = fixture().await;
    let (link, browser) = ready(&f).await;
    let row = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
    let c = row.browser_consent.as_ref().unwrap();
    for (origin, csrf, subject, revision) in [
        (
            "https://attacker.example",
            c.csrf_token.as_str(),
            "owner",
            c.review_revision,
        ),
        (
            "https://cma.example.test",
            "00000000000000000000000000000000",
            "owner",
            c.review_revision,
        ),
        (
            "https://cma.example.test",
            c.csrf_token.as_str(),
            "another",
            c.review_revision,
        ),
        (
            "https://cma.example.test",
            c.csrf_token.as_str(),
            "owner",
            c.review_revision + 1,
        ),
    ] {
        assert_eq!(
            consent(&f, &link, &browser, origin, csrf, subject, revision)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut other = actor();
    other.actor_id = "other".into();
    let other_link = f.auth.ensure_actor_link(&other).await.unwrap();
    assert_eq!(
        consent(
            &f,
            &other_link.link_id,
            &browser,
            "https://cma.example.test",
            &c.csrf_token,
            "owner",
            c.review_revision
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.store
            .actor_link_by_id(&link)
            .await
            .unwrap()
            .unwrap()
            .revision,
        row.revision
    );
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    let activated = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    assert_eq!(
        f.store
            .actor_link_by_id(&link)
            .await
            .unwrap()
            .unwrap()
            .revision,
        activated.revision
    );
    assert_eq!(f.auth.pending_actor_wakeups(100).await.unwrap().len(), 1);
}

#[tokio::test]
async fn actor_browser_expired_and_revoked_links_and_replaced_identity_fail_closed() {
    for scenario in ["expired", "revoked", "replacement", "changed-subject"] {
        let f = fixture().await;
        let (link, browser) = ready(&f).await;
        match scenario {
            "expired" => {
                let mut row = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
                let revision = row.revision;
                row.expires_at = 1;
                row.revision += 1;
                f.store
                    .replace_actor_link(Some(revision), row)
                    .await
                    .unwrap();
            }
            "revoked" => f.auth.revoke_actor_link(&actor(), &link).await.unwrap(),
            "replacement" => {
                let mut state = f
                    .store
                    .subject(&SubjectId::new("owner"))
                    .await
                    .unwrap()
                    .unwrap();
                state.incarnation = "replacement".into();
                f.store.put_subject(state).await.unwrap();
            }
            "changed-subject" => {
                let value = browser.split_once('=').unwrap().1;
                f.store
                    .put_browser_login(crate::auth::persistence::BrowserLogin {
                        id: crate::auth::persistence::SecretDigest::identify(
                            crate::auth::AUTH_COOKIE_DOMAIN,
                            value,
                        ),
                        subject: SubjectId::new("different"),
                        expires_at: SystemTime::now() + Duration::from_secs(100),
                    })
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            approve(&f, &link, &browser).await.status(),
            StatusCode::FORBIDDEN,
            "{scenario}"
        );
        assert!(f.auth.pending_actor_wakeups(100).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn actor_browser_outbox_reclaims_after_crash_preserves_origin_and_rejects_revocation() {
    let f = fixture().await;
    let (link, browser) = ready(&f).await;
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    let item = f.auth.pending_actor_wakeups(1).await.unwrap().remove(0);
    let claim = f
        .auth
        .claim_actor_wakeup(&item.grant_id, &item.notification_id, "replica-a", 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claim.origin.provider_conversation_id,
        "provider-selected-sender-a"
    );
    assert_eq!(claim.origin.source_event.conversation_id, "chat-a");
    assert!(f
        .auth
        .claim_actor_wakeup(&item.grant_id, &item.notification_id, "replica-b", 10)
        .await
        .unwrap()
        .is_none());
    let mut row = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
    let revision = row.revision;
    row.wakeup.as_mut().unwrap().state = ActorWakeupState::Claimed {
        holder: "replica-a".into(),
        lease_until: 1,
    };
    row.revision += 1;
    // This serde round trip models process loss; persisted state is sufficient.
    let row = serde_json::from_slice(&serde_json::to_vec(&row).unwrap()).unwrap();
    f.store
        .replace_actor_link(Some(revision), row)
        .await
        .unwrap();
    let resumed = f
        .auth
        .claim_actor_wakeup(&item.grant_id, &item.notification_id, "replica-b", 10)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.notification_id, claim.notification_id);
    assert!(f.auth.ack_actor_wakeup(&claim, "command-a").await.is_err());
    f.auth.renew_actor_wakeup(&resumed, 30).await.unwrap();
    f.auth
        .ack_actor_wakeup(&resumed, "command-a")
        .await
        .unwrap();
    f.auth
        .ack_actor_wakeup(&resumed, "command-a")
        .await
        .unwrap();
    assert!(f.auth.pending_actor_wakeups(100).await.unwrap().is_empty());
    f.auth.revoke_actor_link(&actor(), &link).await.unwrap();
    assert!(f.auth.current_actor_grant(&link, &actor()).await.is_err());
    assert!(f
        .auth
        .ack_actor_wakeup(&resumed, "command-a")
        .await
        .is_err());
}

#[tokio::test]
async fn actor_browser_duplicate_consent_converges_and_revocation_cancels_pending_work() {
    let f = fixture().await;
    let (link, browser) = ready(&f).await;
    let (a, b) = tokio::join!(approve(&f, &link, &browser), approve(&f, &link, &browser));
    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(b.status(), StatusCode::OK);
    let pending = f.auth.pending_actor_wakeups(100).await.unwrap();
    assert_eq!(pending.len(), 1);
    f.auth.revoke_actor_link(&actor(), &link).await.unwrap();
    assert!(f.auth.pending_actor_wakeups(100).await.unwrap().is_empty());
    assert!(f
        .auth
        .claim_actor_wakeup(&link, &pending[0].notification_id, "replica", 60)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn actor_browser_expired_reply_target_never_admits_a_wakeup() {
    let f = fixture().await;
    let (link, browser) = ready(&f).await;
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    let pending = f.auth.pending_actor_wakeups(1).await.unwrap().remove(0);
    let mut row = f.store.actor_link_by_id(&link).await.unwrap().unwrap();
    let revision = row.revision;
    row.origin.as_mut().unwrap().reply_expires_at = 1;
    row.revision += 1;
    f.store
        .replace_actor_link(Some(revision), row)
        .await
        .unwrap();
    assert!(f
        .auth
        .claim_actor_wakeup(&link, &pending.notification_id, "replica", 60)
        .await
        .unwrap()
        .is_none());
    assert!(f.auth.pending_actor_wakeups(1).await.unwrap().is_empty());
}

#[tokio::test]
async fn actor_browser_return_url_is_not_an_authorization_intent() {
    let f = fixture().await;
    let link = f
        .auth
        .ensure_actor_link_with_origin(&actor(), origin())
        .await
        .unwrap();
    let target = format!("/auth/bot-father/link/{}/complete", link.link_id);
    let response = call(
        &f,
        Request::get(format!("/auth/login?return_to={target}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let url = reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let response = finish(&f, &state, &cookie(&response, "cma_login_")).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let browser = cookie(&response, &format!("{AUTH_COOKIE}="));
    let review = call(
        &f,
        Request::get(target)
            .header(header::COOKIE, browser)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(review.status(), StatusCode::FORBIDDEN);
    let row = f
        .store
        .actor_link_by_id(&link.link_id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.subject.is_none() && row.browser_consent.is_none() && row.wakeup.is_none());
}

#[path = "actor_continuation_tests.rs"]
mod continuation_tests;
