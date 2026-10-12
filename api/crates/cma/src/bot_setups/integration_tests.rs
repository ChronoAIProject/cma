//! Real command/grant services with disposable HTTP identity and gateway peers.
use super::*;
use serde_json::{json, Value};
use std::sync::Mutex;
mod connections;
mod corpus;
mod fixture;
mod ports;
use crate::auth::{actor_links::ActorWakeupState, persistence::AuthStore};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    routing::post,
    Extension, Json, Router,
};
use fixture::*;
use layer1_public::{AuthMethod, DelegatedCredential};
use layer2_core::{
    resource_access::{CallerContext, ResourceActor},
    sandbox::binding::{BindingState, BindingStore, BindingWork},
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

fn capability(notification: &crate::auth::actor_links::ActorWakeupNotification) -> String {
    format!(
        "cma1.cont.{}.{}",
        notification.grant.grant_id, notification.notification_id
    )
}
async fn request(
    f: &Fixture,
    resources: Router,
    path: &str,
    body: Value,
) -> axum::response::Response {
    let mut request = Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(AuthMethod::BindingKey {
        binding_id: BINDING.into(),
    });
    request
        .extensions_mut()
        .insert(DelegatedCredential::new(KEY.into()));
    f.authority
        .delegation_router(resources)
        .oneshot(request)
        .await
        .unwrap()
}
async fn json_body(response: axum::response::Response) -> Value {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(!text.contains(KEY));
    assert!(!text.contains(USER_TOKEN));
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn continuation_reuses_admission_and_reply_identity_after_lost_response_or_ack() {
    for lost_response in [false, true] {
        let f = Fixture::new().await;
        let first = f.activate().await;
        f.peer
            .lose_reply_once
            .store(lost_response, Ordering::SeqCst);
        let result = f.worker().deliver(&first).await;
        assert_eq!(result.is_err(), lost_response);
        // Reconstruct worker as after restart, retaining only committed stores.
        // The first gateway dispatch already happened even if its answer was lost.
        f.authority
            .auth
            .release_actor_wakeup(&first, 0)
            .await
            .unwrap();
        let pending = f
            .authority
            .auth
            .pending_actor_wakeups(1)
            .await
            .unwrap()
            .remove(0);
        let resumed = f
            .authority
            .auth
            .claim_actor_wakeup(
                &pending.grant_id,
                &pending.notification_id,
                "restarted",
                120,
            )
            .await
            .unwrap()
            .unwrap();
        assert_ne!(first.lease_holder, resumed.lease_holder);
        f.worker().run(resumed.clone()).await;
        let row = f
            .auth_store
            .actor_link_by_id(&pending.grant_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            row.wakeup.unwrap().state,
            ActorWakeupState::Acknowledged { .. }
        ));
        assert_eq!(f.peer.effects.lock().unwrap().len(), 1);
        assert_eq!(f.peer.reply_calls.load(Ordering::SeqCst), 2);
        {
            let polled = f.peer.polled.lock().unwrap();
            assert_eq!(polled.len(), 2);
            assert_eq!(polled[0].receipt, polled[1].receipt);
            let admitted = &polled[0].receipt.envelope;
            assert!(
                admitted.command.external_event().is_none(),
                "callback cannot fabricate human consent"
            );
            let content = serde_json::to_string(admitted).unwrap();
            assert!(content.contains(&capability(&first)));
            assert!(!content.contains(USER_TOKEN));
            assert!(!content.contains(KEY));
        }
        assert!(f
            .authority
            .auth
            .pending_actor_wakeups(10)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn continuation_rejects_wrong_origin_and_revoked_or_changed_authority_before_dispatch() {
    let f = Fixture::new().await;
    let notification = f.activate().await;
    for field in [
        "agent",
        "worktree",
        "provider_conversation",
        "reply_target",
        "source",
        "sender",
        "generation",
    ] {
        let mut wrong = notification.clone();
        match field {
            "agent" => wrong.origin.agent_id = layer2_core::AgentId("agt_other".into()),
            "worktree" => wrong.origin.worktree_id = layer2_core::WorktreeId("wkt_other".into()),
            "provider_conversation" => {
                wrong.origin.provider_conversation_id = "other-provider-conversation".into()
            }
            "reply_target" => wrong.origin.reply_target_ref = "cmaeg1.rt.other-user".into(),
            "source" => {
                wrong.origin.source_event.payload["activity"]["actor"]["id"] = "other-user".into()
            }
            "sender" => wrong.actor.actor_id = "other-user".into(),
            _ => wrong.origin.binding_generation += 1,
        }
        assert!(
            matches!(f.worker().deliver(&wrong).await, Err(BotError::Forbidden)),
            "{field}"
        );
    }
    assert!(f.peer.polled.lock().unwrap().is_empty());
    assert_eq!(f.peer.reply_calls.load(Ordering::SeqCst), 0);
    f.authority
        .auth
        .revoke_actor_link(&notification.actor, &notification.grant.grant_id)
        .await
        .unwrap();
    assert!(matches!(
        f.worker().deliver(&notification).await,
        Err(BotError::Forbidden)
    ));
    assert!(f.peer.polled.lock().unwrap().is_empty());
}

#[tokio::test]
async fn continuation_capability_keeps_original_input_without_rechecking_expired_gateway_event() {
    let f = Fixture::new().await;
    let notification = f.activate().await;
    let before = f.peer.gateway_calls.load(Ordering::SeqCst);
    let event = f
        .authority
        .verified_event(
            BINDING,
            KEY,
            &capability(&notification),
            Some(&json!({"kind":{"type":"message","text":"confirm"}})),
        )
        .await
        .unwrap();
    assert_eq!(event.event_id, f.source.event_id);
    assert_eq!(
        event.occurred_at,
        f.source.payload["activity"]["occurred_at"]
    );
    assert!(matches!(event.input, SetupInput::Text(text) if text == "Please help create a bot"));
    assert_eq!(f.peer.gateway_calls.load(Ordering::SeqCst), before);
    let mut retired = f.binding.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    f.store
        .save_binding(Some(f.binding.revision), retired)
        .await
        .unwrap();
    assert!(matches!(
        f.authority
            .verified_event(BINDING, KEY, &capability(&notification), None)
            .await,
        Err(BotError::Forbidden)
    ));
}

#[tokio::test]
async fn delegated_operation_uses_linked_visitor_and_never_returns_either_credential() {
    let f = Fixture::new().await;
    let notification = f.activate().await;
    let calls = Arc::new(AtomicUsize::new(0));
    for echo_secret in [false, true] {
        let observed = calls.clone();
        let grant = notification.grant.grant_id.clone();
        let resources = Router::new().route(
            "/api/v1/sandboxes",
            post(
                move |Extension(context): Extension<CallerContext>,
                      Extension(token): Extension<DelegatedCredential>,
                      headers: axum::http::HeaderMap,
                      Json(body): Json<Value>| {
                    let observed = observed.clone();
                    let grant = grant.clone();
                    async move {
                        observed.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(context.caller.subject.as_str(), "visitor");
                        assert_eq!(
                            context.actor,
                            ResourceActor::Api {
                                subject: layer2_core::SubjectId::new("visitor"),
                                delegation_id: Some(grant)
                            }
                        );
                        assert_eq!(token.value(), USER_TOKEN);
                        assert!(!headers.contains_key(header::AUTHORIZATION));
                        assert_eq!(headers["idempotency-key"], "create-fixture");
                        assert_eq!(body, json!({"name":"visitor-sandbox"}));
                        Json(if echo_secret {
                            json!({"accidental_echo":token.value()})
                        } else {
                            json!({"created":"visitor-sandbox"})
                        })
                    }
                },
            ),
        );
        let response = request(&f, resources, "/api/v1/agent-delegations/operations", json!({"event_ref":capability(&notification),"method":"POST","path":"/api/v1/sandboxes","idempotency_key":"create-fixture","body":{"name":"visitor-sandbox"}})).await;
        assert_eq!(
            response.status(),
            if echo_secret {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            }
        );
        let value = json_body(response).await;
        if !echo_secret {
            assert_eq!(
                value,
                json!({"status":200,"body":{"created":"visitor-sandbox"}})
            );
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let audits = f.audit.0.lock().unwrap();
    assert_eq!(audits.len(), 2);
    let evidence = serde_json::to_string(&*audits).unwrap();
    assert!(evidence.contains(&notification.grant.grant_id));
    assert!(evidence.contains("visitor"));
    assert!(!evidence.contains(USER_TOKEN));
    assert!(!evidence.contains(KEY));
}

#[tokio::test]
async fn delegated_operation_requires_login_valid_source_owner_and_allowlisted_route() {
    let f = Fixture::new().await;
    let body = json!({"event_ref":EVENT,"method":"GET","path":"/api/v1/sandboxes"});
    let response = request(
        &f,
        Router::new(),
        "/api/v1/agent-delegations/operations",
        body,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = json_body(response).await;
    assert_eq!(value["body"]["code"], "user_login_required");
    assert!(value["body"]["login_url"]
        .as_str()
        .unwrap()
        .starts_with("https://cma.example.test/auth/"));
    let notification = f.activate().await;
    for override_body in [
        json!({"owner":{"kind":"person","subject":"father-owner"}}),
        json!({"path":"/api/v1/admin/settings"}),
        json!({"path":"/api/v1/bots"}),
        json!({"path":"https://other.example/api/v1/sandboxes"}),
        json!({"event_ref":format!("cma1.cont.{}.{}", notification.grant.grant_id, "f".repeat(64))}),
    ] {
        let mut body = json!({"event_ref":capability(&notification),"method":"GET","path":"/api/v1/sandboxes"});
        body.as_object_mut()
            .unwrap()
            .extend(override_body.as_object().unwrap().clone());
        let response = request(
            &f,
            Router::new(),
            "/api/v1/agent-delegations/operations",
            body,
        )
        .await;
        assert!(matches!(
            response.status(),
            StatusCode::BAD_REQUEST | StatusCode::FORBIDDEN
        ));
        json_body(response).await;
    }
    assert!(f.audit.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn delegated_organization_creation_never_falls_back_to_personal_ownership() {
    let f = Fixture::new().await;
    let notification = f.activate().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let resources = Router::new().fallback(move |Extension(context): Extension<CallerContext>| {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            assert_eq!(context.caller.subject.as_str(), "visitor");
            assert!(context.validate_principal());
            assert_eq!(context.memberships.len(), 1);
            assert_eq!(context.memberships[0].organization.as_str(), "visitor-org");
            Json(json!({"authorized":true}))
        }
    });
    for path in [
        "/api/v1/sandboxes",
        "/api/v1/agent-profiles",
        "/api/v1/workspace-profiles?view=all",
        "/api/v1/sandbox-profiles",
        "/api/v1/event-worker-profiles",
        "/api/v1/marketplace/agent-profiles/profile-one/fork",
    ] {
        let response = request(&f, resources.clone(), "/api/v1/agent-delegations/operations", json!({
            "event_ref":capability(&notification),"owner":{"kind":"organization","subject":"visitor-org"},
            "method":"POST","path":path,"idempotency_key":"organization-creation"
        })).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        let value = json_body(response).await;
        assert!(value["detail"]
            .as_str()
            .unwrap()
            .contains("personal ownership only"));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(f.audit.0.lock().unwrap().is_empty());
    for (method, path) in [
        ("GET", "/api/v1/sandboxes/organization-sandbox"),
        ("POST", "/api/v2/responses"),
        (
            "POST",
            "/api/v2/agents/organization-agent/responses/response-one/controls",
        ),
    ] {
        let response = request(&f, resources.clone(), "/api/v1/agent-delegations/operations", json!({
            "event_ref":capability(&notification),"owner":{"kind":"organization","subject":"visitor-org"},
            "method":method,"path":path,"idempotency_key":"organization-operation"
        })).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            json_body(response).await,
            json!({"status":200,"body":{"authorized":true}})
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
