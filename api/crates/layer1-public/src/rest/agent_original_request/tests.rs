use super::*;
use crate::rest::{agents_tests as fixture, resource_access_tests::context, shared_agent_tests};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use layer2_core::{
    sandbox::{
        store::{AgentLogStore, AgentStore},
        AgentStatus, FirstMessagePurpose, FirstMessageRecord,
    },
    Authorization, RuntimeCapabilities,
};
use serde_json::{json, Value};
use tower::ServiceExt;

const ID: &str = "agt_original";
const PATH: &str = "/api/v1/agents/agt_original/original-request";
const NOW: &str = "2026-11-20T00:00:00Z";

#[tokio::test]
async fn exact_owner_request_is_private_and_read_only_even_before_dispatch() {
    let fixture = fixture::fixture().await;
    let text = "  task\r\n第二行 🦀\n\tunchanged  ";
    let mut agent = fixture::seeded_agent(ID, AgentStatus::Starting, 2, None);
    agent.conversation.first_message = Some(FirstMessageRecord::new("task", text, NOW).unwrap());
    fixture::insert_agent(&fixture.store, agent, vec![]).await;
    let id = AgentId(ID.into());
    let before = fixture.store.get_agent(&id).await.unwrap();
    let log = fixture.store.agent_log(&id, 0, 200).await.unwrap();
    let response = fixture
        .router
        .clone()
        .oneshot(fixture::request(Method::GET, PATH, "owner-a", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let body: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body,
        json!({"status":"ready","agent_id":ID,"original_request":{"text":text,"source":"admitted_first_task","turn_sequence":null}})
    );
    assert_eq!(fixture.store.get_agent(&id).await.unwrap(), before);
    assert_eq!(fixture.store.agent_log(&id, 0, 200).await.unwrap(), log);
}

#[tokio::test]
async fn missing_setup_and_legacy_purpose_are_unavailable_without_guessing() {
    for purpose in [Some(FirstMessagePurpose::Setup), None] {
        let fixture = fixture::fixture().await;
        let mut agent = fixture::seeded_agent(ID, AgentStatus::Ready, 2, None);
        let mut record = FirstMessageRecord::new("task", "private setup", NOW).unwrap();
        record.purpose = purpose;
        agent.conversation.first_message = Some(record);
        fixture::insert_agent(&fixture.store, agent, vec![]).await;
        let (status, body) = fixture::json_response(
            &fixture.router,
            fixture::request(Method::GET, PATH, "owner-a", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({"status":"unavailable","agent_id":ID,"original_request":null})
        );
    }
    let fixture = fixture::fixture().await;
    fixture::insert_agent(
        &fixture.store,
        fixture::seeded_agent(ID, AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let (status, body) = fixture::json_response(
        &fixture.router,
        fixture::request(Method::GET, PATH, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "unavailable");
}

#[tokio::test]
async fn authentication_history_permission_and_conversation_capability_remain_required() {
    for path in [PATH, "/api/v1/agents/agt_original/reply-preview"] {
        let fixture = fixture::fixture().await;
        let mut agent = fixture::seeded_agent(ID, AgentStatus::Ready, 2, None);
        agent.capabilities = RuntimeCapabilities::default();
        fixture::insert_agent(&fixture.store, agent, vec![]).await;
        let anonymous = fixture
            .router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let mut denied = fixture::request(Method::GET, path, "owner-a", None, None);
        denied.extensions_mut().insert(Authorization::new([]));
        assert_eq!(
            fixture::json_response(&fixture.router, denied).await.0,
            StatusCode::FORBIDDEN
        );
        let unknown = fixture::json_response(
            &fixture.router,
            fixture::request(Method::GET, path, "owner-b", None, None),
        )
        .await;
        assert_eq!(unknown.0, StatusCode::NOT_FOUND);
        let unsupported = fixture::json_response(
            &fixture.router,
            fixture::request(Method::GET, path, "owner-a", None, None),
        )
        .await;
        assert_ne!(unsupported.0, StatusCode::OK);
        assert!(!unsupported.1.to_string().contains("original_request"));
    }
}

#[tokio::test]
async fn shared_history_does_not_grant_admission_body_and_personal_resource_context_works() {
    let (fixture, _, _, _) = shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let id = AgentId("agt_shared_read".into());
    let mut agent = fixture.store.get_agent(&id).await.unwrap().unwrap();
    agent.conversation.first_message =
        Some(FirstMessageRecord::new("task", "private admission body", NOW).unwrap());
    let revision = agent.revision;
    agent.revision += 1;
    fixture.store.save_agent(revision, agent).await.unwrap();
    let path = "/api/v1/agents/agt_shared_read/original-request";
    for (subject, expected) in [
        ("owner-a", StatusCode::OK),
        ("recipient", StatusCode::NOT_FOUND),
    ] {
        let mut request = fixture::request(Method::GET, path, subject, None, None);
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
        let (status, body) = fixture::json_response(&fixture.router, request).await;
        assert_eq!(status, expected, "{body}");
        assert_eq!(
            body.to_string().contains("private admission body"),
            subject == "owner-a"
        );
    }
}

struct ChangingStanding {
    store: std::sync::Arc<layer2_core::sandbox::store::InMemorySandboxStore>,
    calls: std::sync::atomic::AtomicUsize,
    change: &'static str,
}

#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ChangingStanding {
    async fn erasing(
        &self,
        _: &layer2_core::SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 1 && self.change != "account" {
            let mut agent = self
                .store
                .get_agent(&AgentId(ID.into()))
                .await
                .unwrap()
                .unwrap();
            let revision = agent.revision;
            match self.change {
                "capability" => agent.capabilities = RuntimeCapabilities::default(),
                "incarnation" => agent.created_at = "2026-11-21T00:00:00Z".into(),
                "body" => {
                    agent.conversation.first_message = Some(
                        FirstMessageRecord::new("replacement", "replacement body", NOW).unwrap(),
                    )
                }
                _ => panic!("unexpected change"),
            }
            agent.revision += 1;
            self.store.save_agent(revision, agent).await.unwrap();
        }
        Ok(self.change == "account" && call >= 2)
    }
}

#[tokio::test]
async fn release_rechecks_account_capability_incarnation_and_original_source() {
    use layer2_core::resource_access::ResourceAccessService;
    use std::sync::{atomic::AtomicUsize, Arc, Mutex};
    for (preview, change) in [false, true]
        .into_iter()
        .flat_map(|preview| {
            ["account", "capability", "incarnation", "body"].map(move |change| (preview, change))
        })
        .filter(|(preview, change)| !preview || *change != "body")
    {
        let fixture = fixture::fixture().await;
        let mut agent = fixture::seeded_agent(ID, AgentStatus::Ready, 2, None);
        agent.conversation.first_message =
            Some(FirstMessageRecord::new("task", "original private body", NOW).unwrap());
        fixture::insert_agent(&fixture.store, agent, vec![]).await;
        let standing = Arc::new(ChangingStanding {
            store: fixture.store.clone(),
            calls: AtomicUsize::new(0),
            change,
        });
        let access = Arc::new(
            ResourceAccessService::new(
                fixture.store.clone(),
                Arc::new(shared_agent_tests::Authority(Mutex::new(
                    shared_agent_tests::Reply::Missing,
                ))),
            )
            .with_account_standing(standing.clone()),
        );
        let reads = fixture.reads().with_resource_access(access);
        let caller = context("owner-a").caller;
        let result = if preview {
            reads
                .reply_preview(&caller, None, None, &AgentId(ID.into()))
                .await
                .map(|_| ())
        } else {
            reads
                .original_request(&caller, None, None, &AgentId(ID.into()))
                .await
                .map(|_| ())
        };
        assert!(
            result.is_err(),
            "{change} must refuse the buffered private body"
        );
        assert!(standing.calls.load(std::sync::atomic::Ordering::SeqCst) >= 3);
    }
}

pub(crate) async fn contract_scenario(
    name: &str,
) -> Option<crate::rest::public_contract_tests::Scenario> {
    let mode = name.strip_prefix("read_projection_")?;
    let fixture = fixture::fixture().await;
    let mut agent = fixture::seeded_agent(ID, AgentStatus::Ready, 2, None);
    if mode == "owner" {
        agent.conversation.first_message =
            Some(FirstMessageRecord::new("task", "  exact task\r\n第二行 🦀\n", NOW).unwrap());
    }
    fixture::insert_agent(&fixture.store, agent, vec![]).await;
    Some(crate::rest::public_contract_tests::Scenario {
        router: fixture.router,
        subject: Some("owner-a"),
        authorization: Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        symbols: Default::default(),
    })
}
