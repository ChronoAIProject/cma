use super::*;
use crate::auth::actor_links::EventActor;
use crate::auth::AUTH_COOKIE;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt as _;

fn actor() -> EventActor {
    EventActor {
        binding_id: "binding-a".into(),
        channel_id: "channel-a".into(),
        conversation_id: "chat-a".into(),
        actor_id: "sender-a".into(),
        platform: "telegram".into(),
    }
}
async fn fixture() -> Fixture {
    let f = Fixture::new().await;
    *f.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:bot:manage"]}));
    f.seed().await;
    f
}
async fn candidate(f: &Fixture) -> (crate::auth::actor_links::ActorLinkStatus, String) {
    let initial = f.auth.ensure_actor_link(&actor()).await.unwrap();
    // Compatibility fixture: existing encrypted rows predate browser consent.
    let mut legacy = f
        .store
        .actor_link_by_id(&initial.link_id)
        .await
        .unwrap()
        .unwrap();
    legacy.activation = crate::auth::actor_links::ActorActivation::LegacyChatProof;
    legacy.revision += 1;
    assert!(f
        .store
        .replace_actor_link(Some(initial.revision), legacy)
        .await
        .unwrap());
    assert!(initial.candidate.is_none());
    assert!(initial.grant_id.is_none());
    let cookie = f
        .auth
        .issue_browser_login(SubjectId::new("owner"))
        .await
        .unwrap();
    let response = crate::auth::router(f.auth.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/auth/bot-father/link/{}/complete",
                    initial.link_id
                ))
                .header(header::COOKIE, format!("{AUTH_COOKIE}={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let html = String::from_utf8(
        to_bytes(response.into_body(), 65536)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let code = html
        .split("<pre>link_")
        .nth(1)
        .unwrap()
        .split("</pre>")
        .next()
        .unwrap()
        .to_string();
    let status = f.auth.ensure_actor_link(&actor()).await.unwrap();
    assert!(!serde_json::to_string(&status).unwrap().contains(&code));
    assert!(status.grant_id.is_none());
    (status, code)
}

#[path = "actor_browser_tests.rs"]
mod browser_tests;

#[tokio::test]
async fn actor_link_requires_both_browser_and_same_sender_and_replays_without_new_authority() {
    let f = fixture().await;
    let (status, code) = candidate(&f).await;
    let subject = &status.candidate.as_ref().unwrap().subject;
    assert!(f
        .auth
        .credential_for_grant(&status.link_id, &actor())
        .await
        .is_err());
    let mut other = actor();
    other.actor_id = "different-sender".into();
    assert!(f
        .auth
        .confirm_actor_link(&other, status.revision, subject, &code)
        .await
        .is_err());
    let grant = f
        .auth
        .confirm_actor_link(&actor(), status.revision, subject, &code)
        .await
        .unwrap();
    assert_eq!(grant.subject, SubjectId::new("owner"));
    let replay = f
        .auth
        .confirm_actor_link(&actor(), status.revision, subject, &code)
        .await
        .unwrap();
    assert_eq!(grant.grant_id, replay.grant_id);
    assert!(f
        .auth
        .credential_for_grant(&grant.grant_id, &actor())
        .await
        .is_ok());
    assert!(f
        .auth
        .credential_for_grant(&grant.grant_id, &other)
        .await
        .is_err());
    f.auth
        .revoke_actor_link(&actor(), &grant.grant_id)
        .await
        .unwrap();
    assert!(f
        .auth
        .credential_for_grant(&grant.grant_id, &actor())
        .await
        .is_err());
    assert!(f
        .auth
        .confirm_actor_link(&actor(), status.revision, subject, &code)
        .await
        .is_err());
}

#[tokio::test]
async fn actor_link_accepts_a_protocol_sized_sealed_channel_id() {
    let f = fixture().await;
    let mut actor = actor();
    actor.channel_id = format!("cmaeg1.ch.{}", "a".repeat(268));
    assert_eq!(actor.channel_id.len(), 278);

    let status = f.auth.ensure_actor_link(&actor).await.unwrap();
    assert!(status.login_url.is_some());
    assert!(status.grant_id.is_none());
}

#[tokio::test]
async fn actor_link_cannot_borrow_a_replacement_login_even_for_same_subject() {
    let f = fixture().await;
    let (status, code) = candidate(&f).await;
    let grant = f
        .auth
        .confirm_actor_link(&actor(), status.revision, &SubjectId::new("owner"), &code)
        .await
        .unwrap();
    let previous = f.store.subject(&grant.subject).await.unwrap().unwrap();
    let mut replacement = previous.clone();
    replacement.incarnation = "replacement-login".into();
    f.store.put_subject(replacement).await.unwrap();
    assert!(f
        .auth
        .credential_for_grant(&grant.grant_id, &actor())
        .await
        .is_err());
    let renewed = f.auth.ensure_actor_link(&actor()).await.unwrap();
    assert_ne!(renewed.link_id, grant.grant_id);
    assert!(renewed.grant_id.is_none());
    assert!(renewed.login_url.is_some());
}

#[tokio::test]
async fn actor_callback_rejects_api_bearer_and_wrong_browser_before_consuming_intent() {
    let f = fixture().await;
    let status = f.auth.ensure_actor_link(&actor()).await.unwrap();
    for with_bearer in [false, true] {
        let mut request =
            Request::builder().uri(format!("/auth/bot-father/link/{}/complete", status.link_id));
        if with_bearer {
            request = request.header(header::AUTHORIZATION, "Bearer must-not-leak-access-token");
        }
        let response = crate::auth::router(f.auth.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let unchanged = f.auth.ensure_actor_link(&actor()).await.unwrap();
    assert_eq!(unchanged.revision, status.revision);
    assert!(unchanged.candidate.is_none());
}

#[tokio::test]
async fn wrong_confirmation_proof_does_not_activate_and_limits_attempts() {
    let f = fixture().await;
    let (mut status, code) = candidate(&f).await;
    for _ in 0..5 {
        assert!(f
            .auth
            .confirm_actor_link(
                &actor(),
                status.revision,
                &SubjectId::new("owner"),
                "00000000000000000000000000000000"
            )
            .await
            .is_err());
        status = f.auth.ensure_actor_link(&actor()).await.unwrap();
    }
    assert!(f
        .auth
        .confirm_actor_link(&actor(), status.revision, &SubjectId::new("owner"), &code)
        .await
        .is_err());
    assert!(status.grant_id.is_none());
}
