use layer2_core::{sandbox::agent_io::AgentCommand, AgentId};
use serde_json::json;

#[path = "startup_failure_tests.rs"]
mod startup_failure_tests;

#[test]
fn conversation_only_forms_canonicalize_and_aliases_select_the_same_command() {
    for input in [
        json!("Hello world"),
        json!([{"role":"user","content":"Hello world"}]),
        json!([{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello "},{"type":"input_text","text":"world"}]}]),
    ] {
        let parsed = super::parse::parse(
            json!({"conversation":"agt_fixture","stream":true,"input":input}),
            None,
            "same-key".into(),
        )
        .unwrap();
        let alias = super::parse::parse(
            json!({"stream":true,"input":"Hello world"}),
            Some(AgentId("agt_fixture".into())),
            "same-key".into(),
        )
        .unwrap();
        assert_eq!(parsed.agent_id, alias.agent_id);
        assert_eq!(parsed.command, AgentCommand::message("Hello world"));
        assert_eq!(parsed.command, alias.command);
    }
}

use crate::rest::agents_tests as fixtures;
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use futures_util::StreamExt;
use layer2_core::{
    resource_access::*,
    sandbox::{
        agent_io::*,
        store::{AgentStore, InMemorySandboxStore, PageRequest},
        AgentStatus,
    },
    Authorization, Caller, Capability, SubjectId,
};
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tower::ServiceExt;

struct Authority;
pub(crate) fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        credential: CredentialContext {
            issuer: "https://fixture.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "exact-fixture".into(),
        },
        memberships: vec![],
    }
}
#[async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if credential != "fixture-credential"
            || context.caller.subject != SubjectId::new("owner-a")
            || owner
                != &(AgentOwner::Person {
                    subject: context.caller.subject.clone(),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "fixture-custody".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}
#[async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if org != &SubjectId::new("org") || !matches!(caller.subject.as_str(), "owner-a" | "reader")
        {
            return Err(MembershipError::NotMember);
        }
        let mut value = context(caller.subject.as_str());
        let now = SystemTime::now();
        value.memberships.push(OrganizationMembership::new(
            value.credential.clone(),
            org.clone(),
            OrganizationRole::Viewer,
            now,
            now + Duration::from_secs(60),
        )?);
        Ok(value)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}
pub(crate) struct Fixture {
    pub(crate) router: Router,
    pub(crate) store: Arc<InMemorySandboxStore>,
    pub(crate) access: Arc<ResourceAccessService>,
}
pub(crate) async fn fixture() -> Fixture {
    let (state, store) = fixtures::conversation_fixture().await;
    fixtures::insert_agent(
        &store,
        fixtures::seeded_agent("agt_fixture", AgentStatus::Ready, 2, None),
        vec![],
    )
    .await;
    let authority = Arc::new(Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority);
    let reads = state.reads.with_resource_access(access.clone());
    Fixture {
        router: super::router(super::ResponsesV2State::new(
            crate::agent_page::AgentPageOrigin::test(),
            commands,
            reads,
        )),
        store,
        access,
    }
}
fn request(
    method: Method,
    path: &str,
    subject: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> Request<Body> {
    let mut request = fixtures::request(method, path, subject, key, body);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("fixture-credential".into()));
    request
}
async fn read_json(
    router: &Router,
    method: Method,
    path: &str,
    subject: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    fixtures::json_response(
        router,
        request(method, path, subject, Some("same-key"), body),
    )
    .await
}
async fn create(f: &Fixture, alias: bool) -> axum::response::Response {
    let path = if alias {
        "/api/v2/agents/agt_fixture/responses"
    } else {
        "/api/v2/responses"
    };
    f.router.clone().oneshot(request(Method::POST, path, "owner-a", Some("same-key"), Some(json!({"conversation":"agt_fixture","input":"Hello","stream":true,"metadata":{"private":"owner metadata"}})))).await.unwrap()
}
fn events(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn canonical_alias_disconnect_and_retrieval_keep_one_durable_receipt() {
    let f = fixture().await;
    let before = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap();
    let mut hostile = request(
        Method::POST,
        "/api/v2/responses",
        "owner-a",
        Some("same-key"),
        Some(
            json!({"conversation":"agt_fixture","input":"Hello","stream":true,"metadata":{"private":"owner metadata"}}),
        ),
    );
    hostile
        .headers_mut()
        .insert("host", "evil.example".parse().unwrap());
    hostile
        .headers_mut()
        .insert("x-forwarded-host", "evil.example".parse().unwrap());
    let created = f.router.clone().oneshot(hostile).await.unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let id = created.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let command = created.headers()["x-cma-command-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let mut body = created.into_body().into_data_stream();
    let first = events(&body.next().await.unwrap().unwrap());
    assert_eq!(first[0]["type"], "response.created");
    assert_eq!(first[0]["response"]["status"], "queued");
    assert_eq!(first[0]["response"]["id"], id);
    let callback = first[0]["response"]["callback_url"].as_str().unwrap();
    assert_eq!(callback, "https://cma.example/agents/agt_fixture");
    assert!(!callback.contains("resp_"));
    drop(body);
    let retry = create(&f, true).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(retry.headers()["x-cma-response-id"], id);
    assert_eq!(retry.headers()["x-cma-command-id"], command);
    drop(retry);
    let (status, recovered) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "owner-a",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered, first[0]["response"]);
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        f.store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap(),
        before
    );
    let (status, conflict) = read_json(
        &f.router,
        Method::POST,
        "/api/v2/responses",
        "owner-a",
        Some(json!({"conversation":"agt_fixture","input":"changed","stream":true})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "agent_command_key_conflict");
}

#[tokio::test]
async fn field_validation_is_named_and_never_admits_work() {
    let f = fixture().await;
    for (field, value, code) in [
        ("stream", json!(false), "stream_required"),
        (
            "previous_response_id",
            json!("resp_previous"),
            "conversation_previous_response_conflict",
        ),
        ("model", json!("different-model"), "model_mismatch"),
        ("store", json!(false), "store_required"),
        ("tools", json!([]), "responses_field_unsupported"),
        ("instructions", Value::Null, "responses_field_unsupported"),
        ("reasoning", json!({}), "responses_field_unsupported"),
        ("background", json!(true), "responses_field_unsupported"),
        ("actor", json!("owner-a"), "responses_field_unsupported"),
        ("metadata", json!({"bad":2}), "invalid_metadata"),
    ] {
        let mut body = json!({"conversation":"agt_fixture","stream":true,"input":"Hello"});
        body[field] = value;
        let (status, refusal) = read_json(
            &f.router,
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some(body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {refusal}"
        );
        assert_eq!(refusal["error"]["code"], code);
        assert_eq!(refusal["error"]["param"], field);
    }
    let (status, refusal) = read_json(
        &f.router,
        Method::POST,
        "/api/v2/agents/agt_other/responses",
        "owner-a",
        Some(json!({"conversation":"agt_fixture","stream":true,"input":"Hello"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refusal["error"]["code"], "conversation_alias_mismatch");
    assert!(f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn forged_response_ids_and_inaccessible_receipts_have_identical_not_found_errors() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let (status, hidden) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "unrelated",
        None,
    )
    .await;
    let (missing_status, missing) = read_json(
        &f.router,
        Method::GET,
        "/api/v2/responses/resp_missing",
        "unrelated",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing_status, status);
    assert_eq!(missing, hidden);
    let mut forged = request(
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "unrelated",
        None,
        None,
    );
    forged.extensions_mut().insert(context("owner-a"));
    let (status, value) = fixtures::json_response(&f.router, forged).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "authentication_required");
}

#[tokio::test]
async fn replay_ids_are_stable_and_queue_failure_is_durable_and_retrievable() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let (status, bytes) = fixtures::response(
        &f.router,
        request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let full = events(&bytes);
    assert_eq!(full.len(), 2);
    assert_eq!(full[1]["type"], "response.failed");
    let (_, replay) = fixtures::response(
        &f.router,
        request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events?starting_after=0"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(events(&replay), full[1..]);
    assert!(std::str::from_utf8(&replay).unwrap().contains("id: 1"));
    let (_, retrieved) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "owner-a",
        None,
    )
    .await;
    assert_eq!(retrieved, full[1]["response"]);
}

#[tokio::test]
async fn shared_retrieval_redacts_metadata_and_revocation_fences_buffered_frames() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let share = f
        .access
        .change_share(
            &context("owner-a"),
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_fixture".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "share".into(),
            },
        )
        .await
        .unwrap()
        .share;
    let (status, shared) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "reader",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["metadata"], json!({}));
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "reader",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    let failure = body.next().await.unwrap().unwrap();
    let event = &events(&failure)[0];
    assert_eq!(event["type"], "error");
    assert!(event.get("response").is_none());
    assert!(!std::str::from_utf8(&failure).unwrap().contains("id: "));
    assert!(body.next().await.is_none());
    assert_eq!(
        read_json(
            &f.router,
            Method::GET,
            &format!("/api/v2/responses/{id}"),
            "reader",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(start_paused = true)]
async fn idle_shared_stream_expires_without_a_poll_renewing_membership() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    f.access
        .change_share(
            &context("owner-a"),
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_fixture".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "share".into(),
            },
        )
        .await
        .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "reader",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    let event = events(&body.next().await.unwrap().unwrap());
    assert_eq!(event[0]["type"], "error");
    assert!(body.next().await.is_none());
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items[0]
            .status,
        AgentResponseStatus::Queued
    );
}

#[tokio::test]
async fn replay_refuses_ambiguous_cursors_and_invalid_queries_without_changing_work() {
    let f = fixture().await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let command = layer2_core::ids::AgentCommandId(
        response.headers()["x-cma-command-id"]
            .to_str()
            .unwrap()
            .into(),
    );
    drop(response);
    let before = f.store.get_command(&command).await.unwrap();
    for (query, headers) in [
        ("", vec!["0", "1"]),
        ("", vec!["invalid"]),
        ("?starting_after=0", vec!["1"]),
        ("?unknown=true", vec![]),
        ("?starting_after=0&starting_after=1", vec![]),
        ("?starting_after=99", vec![]),
    ] {
        let mut req = request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events{query}"),
            "owner-a",
            None,
            None,
        );
        for header in headers {
            req.headers_mut()
                .append("last-event-id", header.parse().unwrap());
        }
        let (status, body) = fixtures::json_response(&f.router, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "invalid_replay_cursor");
    }
    for query in [
        "?after=missing&before=other",
        "?before=missing",
        "?unknown=true",
    ] {
        let (status, _) = read_json(
            &f.router,
            Method::GET,
            &format!("/api/v2/conversations/agt_fixture/items{query}"),
            "owner-a",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(f.store.get_command(&command).await.unwrap(), before);
}
