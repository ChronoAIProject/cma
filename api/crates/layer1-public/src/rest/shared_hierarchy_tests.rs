use super::{agents_tests as agents, instances_tests as instances};
use super::{
    hierarchy_router,
    resource_access_tests::context,
    shared_agent_tests::{Authority, Reply},
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use layer2_core::{
    resource_access::*,
    sandbox::{store::*, AgentStatus, HierarchyReadStore},
    AgentId, SandboxId, SubjectId,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub(super) const PATHS: [&str; 5] = [
    "/api/v1/sandboxes/sbx_route",
    "/api/v1/sandboxes/sbx_route/workspaces",
    "/api/v1/workspaces/wks_route",
    "/api/v1/workspaces/wks_route/worktrees",
    "/api/v1/worktrees/wkt_agt_contract",
];

pub(super) async fn fixture(
    whole: bool,
    reply: Reply,
) -> (
    instances::Fixture,
    Arc<ResourceAccessService>,
    Arc<Authority>,
    ResourceShare,
) {
    let mut fixture = instances::fixture(4);
    fixture
        .store
        .create_sandbox(agents::sandbox("sbx_route", "owner-a"))
        .await
        .unwrap();
    fixture
        .store
        .create_workspace(agents::workspace("wks_route", "sbx_route", "owner-a"))
        .await
        .unwrap();
    for id in ["agt_contract", "agt_sibling"] {
        let agent = agents::seeded_agent(id, AgentStatus::Ready, 0, None);
        let mut worktree = agents::seeded_worktree(&agent);
        worktree.name = id.into();
        fixture.store.create_worktree(worktree).await.unwrap();
        fixture.store.create_agent(agent).await.unwrap();
    }
    let authority = Arc::new(Authority(Mutex::new(Reply::Member)));
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        authority.clone(),
    ));
    let share = access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: if whole {
                    ResourceSelector::Sandbox {
                        sandbox_id: SandboxId("sbx_route".into()),
                    }
                } else {
                    ResourceSelector::Agent {
                        agent_id: AgentId("agt_contract".into()),
                    }
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "hierarchy-grant".into(),
            },
        )
        .await
        .unwrap()
        .share;
    fixture.state.hierarchy = fixture
        .state
        .hierarchy
        .with_detail_timings(true)
        .with_resource_access(access.clone());
    fixture.router = hierarchy_router(fixture.state.clone());
    *authority.0.lock().unwrap() = reply;
    (fixture, access, authority, share)
}

fn request(path: &str, subject: &str) -> Request<Body> {
    let mut request = agents::request(Method::GET, path, subject, None, None);
    request.extensions_mut().insert(context(subject));
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    request
}

fn assert_safe(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert!(
                    ![
                        "owner",
                        "owner_id",
                        "profile",
                        "runtime",
                        "failure",
                        "source",
                        "path",
                        "branch",
                        "base_commit",
                        "credential",
                        "lifecycle",
                        "origin"
                    ]
                    .contains(&key.as_str()),
                    "private field {key}"
                );
                assert_safe(value);
            }
        }
        Value::Array(values) => values.iter().for_each(assert_safe),
        Value::String(text) => assert!(
            ![
                "owner-a",
                "private-",
                "/home/cma",
                "sbp_route",
                "wkp_route",
                "agp_route",
                "runtime-route"
            ]
            .iter()
            .any(|value| text.contains(value)),
            "{text}"
        ),
        _ => (),
    }
}

#[tokio::test]
async fn whole_sandbox_reader_gets_safe_hierarchy_and_owner_keeps_exact_legacy_json() {
    let (fixture, _, _, _) = fixture(true, Reply::Member).await;
    for (index, path) in PATHS.iter().enumerate() {
        let mut legacy = request(path, "owner-a");
        legacy.extensions_mut().remove::<CallerContext>();
        let old = agents::json_response(&fixture.router, legacy).await;
        let owner = agents::json_response(&fixture.router, request(path, "owner-a")).await;
        assert_eq!(owner.0, StatusCode::OK, "{}", owner.1);
        assert_eq!(owner, old);
        let (status, shared) =
            agents::json_response(&fixture.router, request(path, "recipient")).await;
        assert_eq!(status, StatusCode::OK, "{shared}");
        assert_safe(&shared);
        if let Some(items) = shared.get("items") {
            assert!(items
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["access"] == "shared"));
        } else {
            assert_eq!(shared["access"], "shared");
        }
        if index == 0 || index == 2 {
            assert_eq!(shared["worktree_count"], 2);
            assert_eq!(shared["agent_count"], 2);
        }
    }
}

#[tokio::test]
async fn narrow_grants_membership_loss_and_malformed_context_conceal_every_parent_route() {
    let (narrow, _, _, _) = fixture(false, Reply::Member).await;
    let (whole, access, authority, share) = fixture(true, Reply::Member).await;
    for path in PATHS {
        assert_eq!(
            agents::json_response(&narrow.router, request(path, "recipient"))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let mut malformed = request(path, "owner-a");
        malformed
            .extensions_mut()
            .insert(context("another-principal"));
        assert_eq!(
            agents::json_response(&whole.router, malformed).await.0,
            StatusCode::UNAUTHORIZED
        );
        for (reply, status) in [
            (Reply::Missing, StatusCode::NOT_FOUND),
            (Reply::Outage, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            *authority.0.lock().unwrap() = reply;
            assert_eq!(
                agents::json_response(&whole.router, request(path, "recipient"))
                    .await
                    .0,
                status
            );
            assert_eq!(
                agents::json_response(&whole.router, request(path, "owner-a"))
                    .await
                    .0,
                StatusCode::OK
            );
        }
    }
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "hierarchy-revoke".into(),
            },
        )
        .await
        .unwrap();
    *authority.0.lock().unwrap() = Reply::Member;
    for path in PATHS {
        assert_eq!(
            agents::json_response(&whole.router, request(path, "recipient"))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn organization_sandbox_pages_keep_personal_json_and_safe_shared_counts() {
    let (fixture, _, authority, _) = fixture(true, Reply::Member).await;
    let before = fixture
        .store
        .read_sandbox_hierarchy(&SandboxId("sbx_route".into()))
        .await
        .unwrap();
    let personal =
        agents::json_response(&fixture.router, request("/api/v1/sandboxes", "owner-a")).await;
    let scoped = agents::json_response(
        &fixture.router,
        request("/api/v1/sandboxes?organization=org", "owner-a"),
    )
    .await;
    assert_eq!(personal.0, StatusCode::OK);
    assert_eq!(scoped, personal);
    let (status, shared) = agents::json_response(
        &fixture.router,
        request("/api/v1/sandboxes?organization=org", "recipient"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_safe(&shared);
    assert_eq!(shared["items"][0]["access"], "shared");
    assert_eq!(shared["items"][0]["agent_count"], 2);
    assert!(shared["items"][0].get("workspaces").is_none());
    *authority.0.lock().unwrap() = Reply::Outage;
    assert_eq!(
        agents::json_response(&fixture.router, request("/api/v1/sandboxes", "owner-a")).await,
        personal
    );
    assert_eq!(
        fixture
            .store
            .read_sandbox_hierarchy(&SandboxId("sbx_route".into()))
            .await
            .unwrap(),
        before
    );
}
