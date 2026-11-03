//! Real public file routes composed with authorization and a deterministic runtime port.
use super::{
    super::{
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{agent_filesystem::AgentFileService, store::SandboxStore},
    AgentId,
};
use std::sync::Arc;

#[path = "filesystem_runtime.rs"]
mod runtime;

#[cfg(test)]
#[path = "native_deliverables.rs"]
mod native_deliverables;
#[path = "filesystem_scope_tests.rs"]
mod scope_tests;

#[path = "browser_cases.rs"]
mod browser_cases;
#[path = "native_browser_chain.rs"]
mod native_browser_chain;

#[tokio::test]
async fn browser_public_contract_requires_explicit_owner_enablement_and_current_incarnation() {
    use crate::rest::agents_tests::request;
    use axum::{
        body::to_bytes,
        http::{Method, StatusCode},
    };
    use tower::ServiceExt as _;
    let fixture = scenario("filesystem_owner").await.unwrap();
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/agents/agt_shared_read/environment",
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    let environment: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let incarnation = environment["runtime_incarnation"].as_str().unwrap();
    let path = format!("/api/v1/agents/agt_shared_read/browser?runtime_incarnation={incarnation}&observer=browser-proof");
    let response = fixture
        .router
        .clone()
        .oneshot(request(Method::GET, &path, "owner-a", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let observed: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 400000).await.unwrap()).unwrap();
    assert_eq!(observed["state"], "unsupported");
    assert!(observed["frame"].is_null());
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/agents/agt_shared_read/browser?runtime_incarnation=old&observer=browser-proof",
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let foreign = scenario("filesystem_foreign").await.unwrap();
    let response = foreign
        .router
        .oneshot(request(Method::GET, &path, "foreign", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let viewer = scenario("filesystem_viewer").await.unwrap();
    let response = viewer
        .router
        .oneshot(request(
            Method::PUT,
            "/api/v1/agents/agt_shared_read/browser",
            "link-recipient",
            None,
            Some(serde_json::json!({"runtime_incarnation":incarnation,"enabled":true})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn computer_environment_public_route_preserves_file_scope_and_denies_foreign_readers() {
    use crate::rest::agents_tests::request;
    use axum::{
        body::to_bytes,
        http::{Method, StatusCode},
    };
    use tower::ServiceExt;
    for (name, writable) in [("filesystem_owner", true), ("filesystem_viewer", false)] {
        let fixture = scenario(name).await.unwrap();
        let response = fixture
            .router
            .oneshot(request(
                Method::GET,
                "/api/v1/agents/agt_shared_read/environment",
                fixture.subject.unwrap(),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(body["agent_id"], "agt_shared_read");
        assert_eq!(body["files"]["scope"], "mounted_home");
        assert_eq!(body["files"]["writable"], writable);
        assert_eq!(body["operation_output"]["scope"], "agent_turn_history");
        assert_eq!(body["browser"]["state"], "unsupported");
        assert!(!body.to_string().contains("runtime_address"));
    }
    let fixture = scenario("filesystem_foreign").await.unwrap();
    let response = fixture
        .router
        .oneshot(request(
            Method::GET,
            "/api/v1/agents/agt_shared_read/environment",
            "foreign",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if name.starts_with("browser_") {
        return browser_cases::scenario(name).await;
    }
    let kind = name.strip_prefix("filesystem_")?;
    scenario_with_runtime(name, Arc::new(runtime::Files::new(kind))).await
}

async fn scenario_with_runtime(
    name: &str,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
) -> Option<Scenario> {
    scenario_with_runtime_boot(name, adapter, &"b".repeat(32)).await
}

async fn scenario_with_runtime_boot(
    name: &str,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    boot: &str,
) -> Option<Scenario> {
    let kind = name.strip_prefix("filesystem_")?;
    assert!([
        "owner",
        "viewer",
        "writer",
        "revoked",
        "foreign",
        "anonymous",
        "not_ready",
        "paused",
        "unsupported",
        "runtime_unsupported",
        "runtime_conflict",
        "unavailable",
        "too_large"
    ]
    .contains(&kind));
    let (fixture, access, _, legacy) = fixture(Reply::Member).await;
    // Isolate the typed link: no unrelated organization share may mask revocation.
    access
        .change_share(
            &context("owner-a"),
            Some("request-credential"),
            ShareCommand {
                selector: legacy.selector,
                change: ShareChange::Revoke {
                    share_id: legacy.id,
                },
                expected_share_revision: Some(legacy.revision),
                idempotency_key: "remove-legacy-fixture-share".into(),
            },
        )
        .await
        .unwrap();
    let mut sandbox = fixture
        .store
        .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.runtime.as_mut().unwrap().boot_id = Some(boot.into());
    if matches!(kind, "not_ready" | "unsupported") {
        sandbox.runtime.as_mut().unwrap().boot_id = None;
    }
    if kind != "unsupported" {
        sandbox.lifecycle.home = Some(Box::new(runtime::persistent_home()));
    }
    sandbox.revision += 1;
    if kind == "paused" {
        sandbox.status = layer2_core::sandbox::SandboxStatus::Paused;
    }
    fixture.store.save_sandbox(revision, sandbox).await.unwrap();
    let subject = if matches!(kind, "viewer" | "writer" | "revoked") {
        "link-recipient"
    } else if kind == "foreign" {
        "foreign"
    } else {
        "owner-a"
    };
    if matches!(kind, "viewer" | "writer" | "revoked") {
        let owner = context("owner-a");
        let token = "c".repeat(64);
        let link = access
            .create_agent_share_link(
                &owner,
                Some("request-credential"),
                CreateAgentShareLink {
                    agent_id: AgentId("agt_shared_read".into()),
                    audience: AgentShareAudience::Public,
                    mode: if kind == "viewer" {
                        ShareMode::View
                    } else {
                        ShareMode::ViewWrite
                    },
                    token: token.clone(),
                    idempotency_key: "file-link".into(),
                },
            )
            .await
            .unwrap();
        let redeemed = access
            .redeem_agent_share_link(
                &context(subject),
                Some("request-credential"),
                &link.share.id,
                &token,
            )
            .await
            .unwrap();
        if kind == "revoked" {
            access
                .change_share(
                    &owner,
                    Some("request-credential"),
                    ShareCommand {
                        selector: link.share.selector,
                        change: ShareChange::Revoke {
                            share_id: link.share.id,
                        },
                        expected_share_revision: Some(redeemed.revision),
                        idempotency_key: "file-revoke".into(),
                    },
                )
                .await
                .unwrap();
        }
    }
    let caller = context(subject);
    let service = AgentFileService::new(access, adapter);
    let mut router = super::super::agent_filesystem_router(service);
    if kind != "anonymous" {
        router = router.layer(Extension(caller.clone())).layer(Extension(
            crate::DelegatedCredential::new("request-credential".into()),
        ));
    }
    Some(Scenario {
        router,
        subject: (kind != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
