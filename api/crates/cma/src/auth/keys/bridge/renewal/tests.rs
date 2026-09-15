use super::*;
use crate::auth::keys::bridge::test_fixture::{lease, HUMAN_SECRET};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use layer2_core::{
    bridge::{BridgeConnector, BridgeId, BridgePlatform, BridgeSecretRef},
    SubjectId,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::SystemTime,
};

struct StateData {
    metadata: Mutex<Value>,
    status: Mutex<StatusCode>,
    deletes: AtomicUsize,
}
struct Fixture {
    lease: BridgeControlPlaneLease,
    previous: SandboxCredentialAuthority,
    issuer: KeyIssuer,
    state: Arc<StateData>,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(owner: ResourceOwner) -> Self {
        let lease = lease(
            owner.clone(),
            BridgeId("fixture-bridge".into()),
            BridgeConnector {
                issuer: "https://nyx.example".into(),
                base_url: "https://nyx.example".into(),
                bot_id: "bot-one".into(),
                platform: BridgePlatform::Telegram,
                route_ids: vec!["route-one".into()],
                key_id: "selected-key".into(),
                custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into())
                    .unwrap(),
                callback_url: "https://cma.example/callback".into(),
            },
            1,
        );
        let source = match owner {
            ResourceOwner::Person { .. } => json!({"type":"personal"}),
            ResourceOwner::Organization { subject } => {
                json!({"type":"org","org_id":subject.as_str(),"role":"admin","allowed":true})
            }
        };
        let expires_at = "2099-01-01T00:00:00Z";
        let grant: crate::auth::keys::MintedKeyResponse = serde_json::from_value(json!({
            "id":"key-prior","full_key":"unused","scopes":"proxy read","state_version":4,
            "allowed_service_ids":["service-old"],"allowed_node_ids":["node-old"],"allow_all_services":false,"allow_all_nodes":false
        })).unwrap();
        let previous = SandboxCredentialAuthority {
            generation: 1,
            provider_key_id: grant.id.clone(),
            provider_state_version: grant.state_version,
            issued_at: layer2_core::rfc3339_at(SystemTime::now()),
            expires_at: expires_at.into(),
            grant_fingerprint: crate::auth::keys::provider_grant_fingerprint(&grant).unwrap(),
        };
        let state = Arc::new(StateData {
            metadata: Mutex::new(json!({"id":"key-prior",
            "name":crate::auth::keys::key_name("renewal-test", "sbx_channel_1").unwrap(),"is_active":true,"expires_at":expires_at,
            "state_version":4,"scopes":"proxy read","allowed_service_ids":["service-old"],"allowed_node_ids":["node-old"],
            "allow_all_services":false,"allow_all_nodes":false,"credential_source":source,"purpose":"general","scheduled_write_enabled":false})),
            status: Mutex::new(StatusCode::OK),
            deletes: AtomicUsize::new(0),
        });
        let router = Router::new()
            .route("/api/v1/api-keys/key-prior", get(metadata).delete(delete))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Self {
            lease,
            previous,
            issuer: KeyIssuer::new(http, base_url, "renewal-test"),
            state,
            task,
        }
    }
    async fn revoke(&self) -> Result<(), KeyIssuerError> {
        self.issuer
            .revoke_bridge_sandbox_key(&self.lease, "sbx_channel_1", &self.previous)
            .await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn authorized(headers: &HeaderMap) -> Result<(), StatusCode> {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {HUMAN_SECRET}"))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}
async fn metadata(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    let status = *state.status.lock().unwrap();
    if status != StatusCode::OK {
        return Err(status);
    }
    Ok(Json(state.metadata.lock().unwrap().clone()))
}
async fn delete(
    State(state): State<Arc<StateData>>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    authorized(&headers)?;
    state.deletes.fetch_add(1, Ordering::SeqCst);
    *state.status.lock().unwrap() = StatusCode::NOT_FOUND;
    Ok(StatusCode::NO_CONTENT)
}

#[tokio::test]
async fn scoped_prior_key_revoke_uses_human_lease_for_person_or_org_and_replays_absence() {
    for owner in [
        ResourceOwner::Person {
            subject: SubjectId::new("fixture-person"),
        },
        ResourceOwner::Organization {
            subject: SubjectId::new("fixture-org"),
        },
    ] {
        let f = Fixture::new(owner).await;
        f.revoke().await.unwrap();
        f.revoke().await.unwrap();
        assert_eq!(f.state.deletes.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn scoped_prior_key_delete_refuses_owner_scope_identity_or_state_changes() {
    let f = Fixture::new(ResourceOwner::Organization {
        subject: SubjectId::new("fixture-org"),
    })
    .await;
    let original = f.state.metadata.lock().unwrap().clone();
    for (field, value) in [
        ("id", json!("another-key")),
        ("name", json!("another-sandbox")),
        ("state_version", json!(5)),
        ("expires_at", json!("2099-01-02T00:00:00Z")),
        ("allowed_service_ids", json!(["service-new"])),
        ("allowed_node_ids", json!(["node-new"])),
        ("credential_source", json!({"type":"personal"})),
        (
            "credential_source",
            json!({"type":"org","org_id":"fixture-org","role":"admin","allowed":false}),
        ),
        ("scheduled_write_enabled", json!(true)),
        ("allow_all_services", json!(true)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        *f.state.metadata.lock().unwrap() = changed;
        assert!(f.revoke().await.is_err(), "accepted changed {field}");
    }
    assert_eq!(f.state.deletes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authentication_and_permission_failures_are_not_reconciled_as_key_absence() {
    let f = Fixture::new(ResourceOwner::Person {
        subject: SubjectId::new("fixture-person"),
    })
    .await;
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        *f.state.status.lock().unwrap() = status;
        assert!(f.revoke().await.is_err());
    }
    assert_eq!(f.state.deletes.load(Ordering::SeqCst), 0);
}
