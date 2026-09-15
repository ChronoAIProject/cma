use super::super::{agents_tests, resource_access_tests::context};
use crate::{AuthenticatedSubject, DelegatedCredential};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    bridge::{invoke::*, *},
    resource_access::*,
    sandbox::{store::InMemorySandboxStore, AgentStatus},
    Caller, SubjectId,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;

struct Authority {
    resources: Arc<InMemorySandboxStore>,
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::NotMember)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Authority {
    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        let existing_target = match &config.placement {
            BridgePlacementPolicy::ExistingAgent { target } => Some(
                self.resources
                    .resource_access_snapshot(&ResourceTarget::Agent {
                        agent_id: target.agent_id.clone(),
                    })
                    .await
                    .map_err(|_| BridgeError::Unavailable)?,
            ),
            _ => None,
        };
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: vec![BridgeRouteObservation {
                    route_id: "route".into(),
                    owner: owner.clone(),
                    bot_id: "bot".into(),
                    key_id: "key".into(),
                    enabled: true,
                }],
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 1,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: false,
                exact_profiles_available: false,
                existing_target,
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
    }
}
#[derive(Default)]
struct Memory {
    state: Mutex<(
        BTreeMap<String, BridgeInvokeGrant>,
        BTreeMap<String, BridgeInvokeReceipt>,
    )>,
}
#[async_trait::async_trait]
impl BridgeInvokeStore for Memory {
    async fn grant(
        &self,
        id: &BridgeInvokeGrantId,
    ) -> Result<Option<BridgeInvokeGrant>, BridgeInvokeError> {
        Ok(self.state.lock().unwrap().0.get(id.as_str()).cloned())
    }
    async fn receipt(&self, id: &str) -> Result<Option<BridgeInvokeReceipt>, BridgeInvokeError> {
        Ok(self.state.lock().unwrap().1.get(id).cloned())
    }
    async fn list(
        &self,
        id: &BridgeId,
        after: Option<&BridgeInvokeGrantId>,
        limit: u32,
    ) -> Result<Vec<BridgeInvokeGrant>, BridgeInvokeError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .0
            .values()
            .filter(|g| &g.pin.bridge_id == id && after.is_none_or(|after| &g.id > after))
            .take(limit as usize + 1)
            .cloned()
            .collect())
    }
    async fn apply(
        &self,
        write: BridgeInvokeWrite,
    ) -> Result<BridgeInvokeReceipt, BridgeInvokeError> {
        write.require_current()?;
        let mut state = self.state.lock().unwrap();
        if let Some(prior) = state.1.get(&write.receipt().id) {
            return match_invoke_replay(prior, write.receipt());
        }
        if state.0.get(write.record().id.as_str()) != write.previous() {
            return Err(BridgeInvokeError::Conflict);
        }
        state
            .0
            .insert(write.record().id.as_str().into(), write.record().clone());
        state
            .1
            .insert(write.receipt().id.clone(), write.receipt().clone());
        Ok(write.receipt().clone())
    }
}
pub(in crate::rest) struct Fixture {
    pub definitions: BridgeService,
    pub bridge_store: Arc<MemoryBridgeStore>,
    pub router: Router,
    pub service: Arc<BridgeInvokeService>,
    pub id: String,
}
pub(in crate::rest) async fn fixture() -> Fixture {
    let resources = agents_tests::fixture().await.store;
    agents_tests::insert_agent(
        &resources,
        agents_tests::seeded_agent("agt_invoke", AgentStatus::Ready, 0, None),
        vec![],
    )
    .await;
    let authority = Arc::new(Authority {
        resources: resources.clone(),
    });
    let bridge_store = Arc::new(MemoryBridgeStore::default());
    let definitions =
        BridgeService::new(bridge_store.clone(), authority.clone(), authority.clone());
    let id = BridgeService::creation_id(&context("owner-a"), "invoke-bridge").unwrap();
    let config:BridgeConfig=serde_json::from_value(json!({
        "display_name":"Retained grant fixture","connector":{"issuer":"https://identity.test","base_url":"https://identity.test","bot_id":"bot","platform":"telegram",
            "route_ids":["route"],"key_id":"key","custody":"brsec_00000000000000000000000000000001","callback_url":"https://cma.example/bridge"},
        "partition":"fixed_shared_agent","placement":{"kind":"existing_agent","target":issue_body()["target"]},"admission":{"kind":"all"},"reply":"final_response",
        "limits":{"max_partitions":1,"max_sandboxes":1,"max_queued_messages":10,"max_messages_per_minute":30,"max_payload_bytes":4096,"inbox_retention_seconds":3600,"reply_secret_retention_seconds":600}
    })).unwrap();
    definitions
        .mutate(
            &context("owner-a"),
            None,
            BridgeMutation {
                id: id.clone(),
                expected_revision: 0,
                idempotency_key: "invoke-bridge".into(),
                change: BridgeChange::Create {
                    owner: ResourceOwner::Person {
                        subject: SubjectId::new("owner-a"),
                    },
                    config: Box::new(config),
                },
            },
        )
        .await
        .unwrap();
    let service = Arc::new(BridgeInvokeService::new(
        definitions.clone(),
        Arc::new(ResourceAccessService::new(resources.clone(), authority)),
        resources,
        Arc::new(Memory::default()),
    ));
    Fixture {
        definitions,
        bridge_store,
        router: super::super::bridge_invoke_router(Some(service.clone())),
        service,
        id: id.as_str().into(),
    }
}
pub(in crate::rest) fn issue_body() -> Value {
    json!({"expected_bridge_revision":1,"target":{"sandbox_id":"sbx_route","workspace_id":"wks_route","worktree_id":"wkt_agt_invoke","agent_id":"agt_invoke"},"shared_context":"use_retained_agent_context"})
}
pub(in crate::rest) fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key)
    }
    let mut request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let caller = context(subject);
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(caller.caller.subject.clone()));
    request
        .extensions_mut()
        .insert(caller.caller.authorization.clone());
    request.extensions_mut().insert(caller);
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("request-credential".into()));
    request
}
pub(in crate::rest) async fn send(
    router: &Router,
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(method, path, subject, key, body))
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let bytes = to_bytes(response.into_body(), 32768).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
