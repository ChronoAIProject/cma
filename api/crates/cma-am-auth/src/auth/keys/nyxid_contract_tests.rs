//! Scope-plan/key HTTP contracts use AuthState's production 15-second client.
use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Exchange, Peer};
use axum::http::{header, Method};
use serde_json::{json, Value};
use std::sync::Arc;

fn issuer(origin: &str) -> Arc<KeyIssuer> {
    let auth = crate::auth::test_support::contract_auth(origin);
    Arc::new(KeyIssuer::new(auth.http.clone(), origin, "contract"))
}

fn mint(issuer: Arc<KeyIssuer>) -> tokio::task::JoinHandle<Result<MintedKey, KeyIssuerError>> {
    tokio::spawn(async move {
        issuer
            .mint(
                "sbx_contract",
                &layer2_core::SubjectId::new("contract-owner"),
                "contract-owner-bearer",
                &[ServiceId::new("chrono-llm-public")],
            )
            .await
    })
}

fn assert_stage(request: &Exchange, stage: usize) {
    assert_eq!(
        request.headers[header::AUTHORIZATION],
        "Bearer contract-owner-bearer"
    );
    assert!(request.uri.query().is_none());
    assert_eq!(
        (request.method.clone(), request.uri.path()),
        match stage {
            0 => (Method::GET, "/api/v1/user-services"),
            1 => (Method::POST, "/api/v1/api-keys/scope-plan"),
            2 => (Method::POST, "/api/v1/api-keys"),
            _ => unreachable!(),
        }
    );
    if stage == 1 {
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap(),
            json!({"selected_service_ids":["contract-service"]})
        );
    }
    if stage == 2 {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["name"], "cma:contract:sbx_contract");
        assert_eq!(body["scopes"], "proxy read");
        assert_eq!(body["allowed_service_ids"], json!(["contract-service"]));
        assert_eq!(body["allowed_node_ids"], json!(["contract-node"]));
        assert_eq!(body["allow_all_services"], false);
        assert_eq!(body["allow_all_nodes"], false);
        assert!(body["scope_plan_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }
}

fn succeed(request: Exchange, stage: usize) {
    match stage {
        0 => request
            .json(json!([{"id":"contract-service","slug":"chrono-llm-public","is_active":true}])),
        1 => {
            let body = serde_json::from_slice(&request.body).unwrap();
            request.json(scope_plan_fixture(
                "contract-owner",
                &body,
                &["contract-node"],
            ));
        }
        2 => request.json(
            json!({"id":"contract-provider-key","full_key":"contract-scoped-fixture",
            "scopes":"proxy read","state_version":7,"allowed_service_ids":["contract-service"],
            "allowed_node_ids":["contract-node"],"allow_all_services":false,"allow_all_nodes":false,
            "allow_auto_connected_services":false}),
        ),
        _ => unreachable!(),
    }
}

async fn reach(peer: &mut Peer, stage: usize) -> Exchange {
    for prior in 0..stage {
        let request = peer.next().await;
        assert_stage(&request, prior);
        succeed(request, prior);
    }
    let request = peer.next().await;
    assert_stage(&request, stage);
    request
}

#[tokio::test]
async fn scope_plan_and_mint_preserve_exact_grant_then_revoke_without_replay() {
    let mut peer = Peer::start().await;
    let issuer = issuer(&peer.origin);
    let task = mint(issuer.clone());
    for stage in 0..3 {
        let request = peer.next().await;
        assert_stage(&request, stage);
        succeed(request, stage);
    }
    let issued = finish(task).await.unwrap();
    assert_eq!(issued.id, "contract-provider-key");
    assert_eq!(issued.state_version, 7);
    assert_eq!(issued.value.expose(), "contract-scoped-fixture");
    assert_eq!(peer.count(), 3);
    let task =
        tokio::spawn(async move { issuer.revoke(&issued.id, "contract-owner-bearer").await });
    let revoke = peer.next().await;
    assert_eq!(revoke.method, Method::DELETE);
    assert_eq!(revoke.uri.path(), "/api/v1/api-keys/contract-provider-key");
    assert_eq!(
        revoke.headers[header::AUTHORIZATION],
        "Bearer contract-owner-bearer"
    );
    revoke.respond(204, "");
    finish(task).await.unwrap();
    assert_eq!(peer.count(), 4);
}

#[tokio::test]
async fn every_issuance_stage_503_stops_with_typed_refusal_and_no_retry_or_later_write() {
    for stage in 0..3 {
        let mut peer = Peer::start().await;
        let task = mint(issuer(&peer.origin));
        reach(&mut peer, stage)
            .await
            .respond(503, "synthetic unavailable");
        assert!(matches!(
            finish(task).await,
            Err(KeyIssuerError::Refused(503))
        ));
        assert_eq!(peer.count(), stage + 1);
    }
}

#[tokio::test]
async fn dispatch_distinguishes_http_refusal_from_unreadable_success() {
    for (status, phase) in [(503, "response_status"), (201, "response_decode")] {
        let mut peer = Peer::start().await;
        let issuer = issuer(&peer.origin);
        let task = tokio::spawn(async move {
            issuer
                .prepare_mint(
                    "sbx_contract",
                    &layer2_core::SubjectId::new("contract-owner"),
                    "contract-owner-bearer",
                    &[ServiceId::new("chrono-llm-public")],
                )
                .await
                .unwrap()
                .dispatch()
                .await
        });
        reach(&mut peer, 2).await.respond(status, "fixture");
        let failure = finish(task).await.unwrap_err();
        assert_eq!(failure.phase, phase);
        assert_eq!(failure.http_status, Some(status));
        assert_eq!(peer.count(), 3);
    }
}

#[tokio::test]
async fn stalled_headers_and_bodies_at_every_stage_obey_the_production_deadline() {
    for stage in 0..3 {
        for body in [false, true] {
            let mut peer = Peer::start().await;
            let task = mint(issuer(&peer.origin));
            let request = reach(&mut peer, stage).await;
            let held = if body {
                request.stall_body();
                None
            } else {
                Some(request)
            };
            tokio::time::pause();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            tokio::time::advance(Duration::from_secs(15)).await;
            assert!(matches!(
                finish(task).await,
                Err(KeyIssuerError::Unavailable(_))
            ));
            assert_deadline(deadline);
            tokio::time::advance(Duration::from_secs(60)).await;
            tokio::task::yield_now().await;
            assert_eq!(peer.count(), stage + 1);
            drop(held);
            tokio::time::resume();
        }
    }
}

#[tokio::test]
async fn revoke_outage_is_never_reported_as_confirmed_success() {
    for timeout in [false, true] {
        let mut peer = Peer::start().await;
        let issuer = issuer(&peer.origin);
        let task = tokio::spawn(async move {
            issuer
                .revoke("contract-provider-key", "contract-owner-bearer")
                .await
        });
        let request = peer.next().await;
        if timeout {
            tokio::time::pause();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            tokio::time::advance(Duration::from_secs(15)).await;
            assert!(matches!(
                finish(task).await,
                Err(KeyIssuerError::Unavailable(_))
            ));
            assert_deadline(deadline);
            drop(request);
            tokio::time::resume();
        } else {
            request.respond(503, "synthetic unavailable");
            assert!(matches!(
                finish(task).await,
                Err(KeyIssuerError::Refused(503))
            ));
        }
        assert_eq!(peer.count(), 1);
    }
}
