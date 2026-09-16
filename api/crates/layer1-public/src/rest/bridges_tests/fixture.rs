use super::super::{agents_tests, resource_access_tests::context};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    bridge::{setup::*, *},
    resource_access::*,
    Caller, SubjectId,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

pub(in crate::rest) struct Ports {
    pub management: std::sync::atomic::AtomicBool,
    pub role: AtomicUsize,
    pub calls: AtomicUsize,
    pub validation: AtomicUsize,
}
#[async_trait::async_trait]
impl OrganizationAuthority for Ports {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let role = match self.role.load(Ordering::SeqCst) {
            0 => return Err(MembershipError::NotMember),
            1 => OrganizationRole::Member,
            2 => OrganizationRole::Admin,
            _ => return Err(MembershipError::Unavailable),
        };
        let mut value = context(caller.subject.as_str());
        let now = SystemTime::now();
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            role,
            now,
            now + Duration::from_secs(30),
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}
#[async_trait::async_trait]
impl BridgeConnectionValidator for Ports {
    fn check_feature(&self, feature: BridgeFeature, _: &BridgePlatform) -> Result<(), BridgeError> {
        if feature == BridgeFeature::Management && !self.management.load(Ordering::SeqCst) {
            return Err(BridgeError::Unavailable);
        }
        Ok(())
    }

    async fn validate_connection(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeValidation, BridgeError> {
        if self.validation.load(Ordering::SeqCst) == 0 {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::RenewableAuthorityUnavailable,
            ));
        }
        BridgeValidation::from_observation(
            owner,
            config,
            BridgeConnectionObservation {
                connector: config.connector.clone(),
                bot_owner: owner.clone(),
                key_owner: owner.clone(),
                routes: config
                    .connector
                    .route_ids
                    .iter()
                    .map(|id| BridgeRouteObservation {
                        route_id: id.clone(),
                        owner: owner.clone(),
                        bot_id: config.connector.bot_id.clone(),
                        key_id: config.connector.key_id.clone(),
                        enabled: true,
                    })
                    .collect(),
                custody_owner: owner.clone(),
                custody_revision: 1,
                key_state_version: 1,
                key_active: true,
                custody_available: true,
                callback_conflict: false,
                renewable_sandbox_authority: true,
                exact_profiles_available: true,
                existing_target: None,
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        )
    }
}
#[async_trait::async_trait]
impl BridgeSetupCredentials for Ports {
    async fn choices(
        &self,
        _: &CallerContext,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<BridgeConnectorChoices, BridgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(BridgeConnectorChoices {
            bots: vec![],
            keys: vec![],
            routes: vec![],
        })
    }
    async fn status(
        &self,
        _: &BridgeService,
        _: &CallerContext,
        _: Option<&str>,
        _: &BridgeId,
        _: u64,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(BridgeCredentialStatus {
            revision: 0,
            present: false,
            revoked: false,
        })
    }
    async fn import(
        &self,
        _: &BridgeService,
        _: &CallerContext,
        _: Option<&str>,
        request: BridgeCredentialImport,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.material.expose() != "fixture-selected-secret" {
            return Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::BindingMismatch,
            ));
        }
        Ok(BridgeCredentialStatus {
            revision: 1,
            present: true,
            revoked: false,
        })
    }
    async fn revoke(
        &self,
        _: &BridgeService,
        _: &CallerContext,
        _: Option<&str>,
        _: BridgeCredentialRevocation,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(BridgeCredentialStatus {
            revision: 2,
            present: true,
            revoked: true,
        })
    }
}
pub(in crate::rest) fn fixture() -> (Router, Arc<MemoryBridgeStore>, Arc<Ports>) {
    let store = Arc::new(MemoryBridgeStore::default());
    let ports = Arc::new(Ports {
        management: std::sync::atomic::AtomicBool::new(true),
        role: AtomicUsize::new(2),
        calls: AtomicUsize::new(0),
        validation: AtomicUsize::new(0),
    });
    let definitions = BridgeService::new(store.clone(), ports.clone(), ports.clone());
    let policy = BridgeSetupPolicy::new(
        "https://identity.test".into(),
        "https://identity.test".into(),
        "https://cma.example/api/v1/bridge-callbacks".into(),
    )
    .unwrap();
    (
        super::super::bridge_router(Some(Arc::new(BridgeSetupService::new(
            definitions,
            policy,
            ports.clone(),
        )))),
        store,
        ports,
    )
}
pub(in crate::rest) fn configuration() -> Value {
    json!({"display_name":"Disposable Bridge","binding":{"bot_id":"bot","platform":"telegram","route_ids":["route-b","route-a"],"key_id":"key"},"partition":"conversation_sender","placement":{"kind":"new_sandbox","sandbox_profile":{"id":"sbp-1","revision":1},"workspace_profile":{"id":"wkp-1","revision":2},"agent_profile":{"id":"agp-1","revision":3}},"admission":{"kind":"scoped","policy":{"senders":{"kind":"allowlist","ids":["sender"]},"chats":{"kind":"open"},"groups":"deny"}},"reply":"disabled","limits":{"max_partitions":2,"max_sandboxes":2,"max_queued_messages":4,"max_messages_per_minute":60,"max_payload_bytes":4096,"inbox_retention_seconds":3600,"reply_secret_retention_seconds":600}})
}
pub(in crate::rest) fn create_body(organization: bool) -> Value {
    json!({"owner":{"kind":if organization{"organization"}else{"person"},"subject":if organization{"org"}else{"owner"}},"config":configuration()})
}
pub(in crate::rest) fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = agents_tests::request(method, path, subject, key, body);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
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
    agents_tests::json_response(router, request(method, path, subject, key, body)).await
}
pub(in crate::rest) async fn create(router: &Router, organization: bool) -> String {
    let (status, value) = send(
        router,
        Method::POST,
        "/api/v1/bridges",
        "owner",
        Some("create-one"),
        Some(create_body(organization)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    value["bridge_id"].as_str().unwrap().into()
}
