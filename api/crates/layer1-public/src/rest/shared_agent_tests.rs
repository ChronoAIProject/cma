use super::agents_tests::{self, json_response};
use super::resource_access_tests::context;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use layer2_core::resource_access::*;
use layer2_core::sandbox::{AgentLogKind, AgentOrigin, AgentStatus, TypedFailure};
use layer2_core::{AgentId, Caller, SubjectId};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};

#[path = "shared_failure_tests.rs"]
mod provider_failure_tests;

pub(super) enum Reply {
    Member,
    Missing,
    Outage,
}
pub(super) struct Authority(pub Mutex<Reply>);
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(credential, "request-credential");
        match *self.0.lock().unwrap() {
            Reply::Missing => return Err(MembershipError::NotMember),
            Reply::Outage => return Err(MembershipError::Unavailable),
            Reply::Member => (),
        }
        let mut context = context(caller.subject.as_str());
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            org.clone(),
            OrganizationRole::Viewer,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        credential: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        assert_eq!(credential, "request-credential");
        match *self.0.lock().unwrap() {
            Reply::Outage => return Err(MembershipError::Unavailable),
            Reply::Missing => return Ok(Vec::new()),
            Reply::Member => (),
        }
        Ok(vec![
            OrganizationChoice {
                id: SubjectId::new("org-two"),
                role: OrganizationRole::Member,
                display_name: None,
            },
            OrganizationChoice {
                id: SubjectId::new("org"),
                role: OrganizationRole::Viewer,
                display_name: Some("Example organization".into()),
            },
        ])
    }
}

pub(super) async fn fixture(
    reply: Reply,
) -> (
    agents_tests::Fixture,
    Arc<ResourceAccessService>,
    Arc<Authority>,
    ResourceShare,
) {
    fixture_with_origin(reply, AgentOrigin::Manual).await
}

pub(super) async fn fixture_with_origin(
    reply: Reply,
    origin: AgentOrigin,
) -> (
    agents_tests::Fixture,
    Arc<ResourceAccessService>,
    Arc<Authority>,
    ResourceShare,
) {
    let authority = Arc::new(Authority(Mutex::new(Reply::Member)));
    let fixture = agents_tests::fixture_with_resource_access(authority.clone()).await;
    let mut agent = agents_tests::seeded_agent("agt_shared_read", AgentStatus::Ready, 9, None);
    agent.origin = origin;
    agent.conversation.conversation_id = Some("private-native-id".into());
    agents_tests::insert_agent(
        &fixture.store,
        agent,
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: Some("private-origin".into()),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 3,
                metadata: [("private".into(), "private-metadata".into())].into(),
            },
            AgentLogKind::OwnerMessage {
                content: "Please review".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Review complete".into(),
            },
            AgentLogKind::SignInRequired {
                url: "https://private-sign-in.test".into(),
                code: Some("private-code".into()),
                message: None,
            },
            AgentLogKind::Usage {
                input_tokens: 5,
                output_tokens: 8,
                context_used: 13,
                context_window: None,
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: "runtime_failed".into(),
                    detail: "private-host and private-path".into(),
                },
            },
        ],
    )
    .await;
    agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_sibling", AgentStatus::Ready, 2, None),
        Vec::new(),
    )
    .await;
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        authority.clone(),
    ));
    let share = access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_shared_read".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "fixture-grant".into(),
            },
        )
        .await
        .unwrap()
        .share;
    *authority.0.lock().unwrap() = reply;
    (fixture, access, authority, share)
}
fn request(path: &str, subject: &str) -> Request<Body> {
    let mut request = agents_tests::request(axum::http::Method::GET, path, subject, None, None);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    request
}

#[tokio::test]
async fn v1_event_page_keeps_the_cursor_and_final_message_for_incremental_text() {
    let fixture = agents_tests::fixture().await;
    let agent = agents_tests::seeded_agent("agt_incremental", AgentStatus::Ready, 7, None);
    agents_tests::insert_agent(
        &fixture.store,
        agent,
        vec![
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                model: None,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Prompt".into(),
            },
            AgentLogKind::AgentMessageDelta {
                offset: 0,
                content: "Reply".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Reply".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    let (status, page) = json_response(
        &fixture.router,
        agents_tests::request(
            axum::http::Method::GET,
            "/api/v1/agents/agt_incremental/events?after=4&limit=2",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["next"], 6);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["seq"], 5);
    assert_eq!(items[0]["kind"], json!({"type":"access_redacted"}));
    assert_eq!(items[1]["seq"], 6);
    assert_eq!(
        items[1]["kind"],
        json!({"type":"agent_message","content":"Reply"})
    );
}

#[tokio::test]
async fn owner_json_is_unchanged_and_shared_documents_contain_only_authorized_fields() {
    use layer2_core::sandbox::store::AgentLogStore;
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    let readability =
        super::agents::agent_router(fixture.state.clone().with_sandbox_profile_readability(
            Arc::new(layer2_core::profiles::InMemoryProfileStore::default()),
        ));
    let target = ResourceTarget::Agent {
        agent_id: AgentId("agt_shared_read".into()),
    };
    let before = fixture
        .store
        .resource_access_snapshot(&target)
        .await
        .unwrap();
    let log = fixture
        .store
        .agent_log(&AgentId("agt_shared_read".into()), 0, 200)
        .await
        .unwrap();
    for suffix in [
        "",
        "/events",
        "/transcript",
        "/transcript?limit=2",
        "/usage",
        "/resources",
    ] {
        let path = format!("/api/v1/agents/agt_shared_read{suffix}");
        let mut old = request(&path, "owner-a");
        old.extensions_mut().remove::<CallerContext>();
        let legacy = json_response(&fixture.router, old).await;
        let current = json_response(&fixture.router, request(&path, "owner-a")).await;
        assert_eq!(current.0, StatusCode::OK);
        assert_eq!(legacy, current, "{suffix}");
        let (status, body) = json_response(&fixture.router, request(&path, "recipient")).await;
        if suffix == "/resources" {
            assert_eq!(status, StatusCode::FORBIDDEN);
            continue;
        }
        assert_eq!(status, StatusCode::OK, "{body}");
        let encoded = body.to_string();
        for private in [
            "private-",
            "sbx_route",
            "wks_route",
            "/home/cma",
            "agp_route",
        ] {
            assert!(!encoded.contains(private), "{encoded}");
        }
        if suffix.is_empty() {
            let enabled = json_response(&readability, request(&path, "recipient")).await;
            assert_eq!(enabled, (status, body.clone()));
            assert_eq!(body["access"], "shared");
            assert!(body.get("profile").is_none());
            assert!(body.get("worktree").is_none());
            assert!(body.get("sandbox").is_none());
        }
    }
    let (_, body) = json_response(
        &fixture.router,
        request("/api/v1/agents/agt_shared_read/events?limit=2", "recipient"),
    )
    .await;
    assert_eq!(
        body["items"],
        json!([
            {"seq":1,"appended_at":"2026-09-05T00:00:00Z","kind":{"type":"access_redacted"}},
            {"seq":2,"appended_at":"2026-09-05T00:00:00Z","kind":{"type":"access_redacted"}}
        ])
    );
    assert_eq!(body["next"], 2);
    assert_eq!(
        fixture
            .store
            .resource_access_snapshot(&target)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        fixture
            .store
            .agent_log(&AgentId("agt_shared_read".into()), 0, 200)
            .await
            .unwrap(),
        log
    );
}

#[tokio::test]
async fn revoked_missing_or_unverifiable_membership_hides_all_shared_reads() {
    let (fixture, access, authority, share) = fixture(Reply::Missing).await;
    for (reply, expected) in [
        (Reply::Missing, StatusCode::NOT_FOUND),
        (Reply::Outage, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        *authority.0.lock().unwrap() = reply;
        for suffix in [
            "",
            "/events",
            "/transcript",
            "/transcript?limit=2",
            "/usage",
            "/resources",
        ] {
            let (status, body) = json_response(
                &fixture.router,
                request(
                    &format!("/api/v1/agents/agt_shared_read{suffix}"),
                    "recipient",
                ),
            )
            .await;
            assert_eq!(status, expected, "{body}");
            if expected == StatusCode::SERVICE_UNAVAILABLE {
                assert_eq!(body["code"], "organization_authority_unavailable");
            }
        }
    }
    *authority.0.lock().unwrap() = Reply::Member;
    assert_eq!(
        json_response(
            &fixture.router,
            request("/api/v1/agents/agt_sibling", "recipient")
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    access
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
    assert_eq!(
        json_response(
            &fixture.router,
            request("/api/v1/agents/agt_shared_read", "recipient")
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn malformed_context_never_falls_back_to_legacy_owner_authority() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    for suffix in [
        "",
        "/events",
        "/transcript",
        "/transcript?limit=2",
        "/usage",
        "/resources",
    ] {
        let mut request = request(
            &format!("/api/v1/agents/agt_shared_read{suffix}"),
            "owner-a",
        );
        request
            .extensions_mut()
            .insert(context("another-principal"));
        let (status, body) = json_response(&fixture.router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "not_authenticated");
    }
}

#[tokio::test]
async fn transcript_route_validates_queries_and_returns_source_continuations() {
    let (fixture, _, _, _) = fixture(Reply::Member).await;
    for suffix in [
        "?limit=0",
        "?limit=33",
        "?turn=3",
        "?limit=2&turn=3&after=3&through=1",
    ] {
        assert_ne!(
            json_response(
                &fixture.router,
                request(
                    &format!("/api/v1/agents/agt_shared_read/transcript{suffix}"),
                    "recipient"
                )
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let (status, page) = json_response(
        &fixture.router,
        request(
            "/api/v1/agents/agt_shared_read/transcript?limit=2",
            "recipient",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["next"]["turn"], 3);
    assert_eq!(page["next"]["after"], 4);
    assert_eq!(page["next"]["through"], page["through_seq"]);
    let (status, continuation) = json_response(
        &fixture.router,
        request(
            &format!(
                "/api/v1/agents/agt_shared_read/transcript?limit=2&turn=3&after=4&through={}",
                page["through_seq"]
            ),
            "recipient",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        continuation["turns"][0]["messages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
