use super::{
    agents_tests,
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    sandbox::{
        channel_usage::*,
        store::{AgentChannelUsageStore, InMemorySandboxStore},
    },
    AgentId,
};
use serde_json::{json, Value};

const BASE: &str = "/api/v1/agents/agt_shared_read/channel-usage";
fn request(subject: &str, verified: bool, suffix: &str) -> Request<Body> {
    let mut request =
        agents_tests::request(Method::GET, &format!("{BASE}{suffix}"), subject, None, None);
    if verified {
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
    }
    request
}
pub(super) async fn append(
    store: &InMemorySandboxStore,
    key: &str,
    change: ChannelUsageChange,
    reason: ChannelBindingChangeReason,
) {
    let snapshot = store
        .agent_channel_usage_snapshot(&AgentId("agt_shared_read".into()))
        .await
        .unwrap();
    store
        .record_agent_channel_usage(ChannelUsageMutation {
            policy_fence: snapshot.provenance.resource.fence(),
            creation: snapshot.state.creation,
            expected_revision: snapshot.state.revision,
            key_sha256: layer2_core::profiles::content_digest(&key).unwrap(),
            actor: context("owner-a").actor,
            change,
            reason,
            at: "2026-09-15T00:00:00Z".into(),
        })
        .await
        .unwrap();
}
pub(super) async fn attach(store: &InMemorySandboxStore) {
    append(
        store,
        "attach",
        ChannelUsageChange::Attach {
            binding: ChannelBindingReference {
                binding_id: "binding-one".into(),
                bridge_id: "bridge-one".into(),
                partition_id: "partition-one".into(),
                generation: 1,
                previous_binding_id: None,
            },
        },
        ChannelBindingChangeReason::Attachment,
    )
    .await;
}
async fn response(
    router: &Router,
    subject: &str,
    verified: bool,
    suffix: &str,
) -> (StatusCode, Value) {
    agents_tests::json_response(router, request(subject, verified, suffix)).await
}

#[tokio::test]
async fn channel_usage_routes_keep_legacy_owner_reads_and_redact_shared_binding_details() {
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let router = super::agent_channel_usage_router(AgentChannelUsageService::new(
        fixture.store.clone(),
        access,
    ));
    for verified in [false, true] {
        let (status, value) = response(&router, "owner-a", verified, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            value,
            json!({"agent_id":"agt_shared_read","channel_usage":{"active":false,"has_history":false,"first_used_at":null,"last_used_at":null},"bindings":[]})
        );
    }
    attach(&fixture.store).await;
    let (status, owner) = response(&router, "owner-a", true, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(owner["bindings"][0]["binding_id"], "binding-one");
    let (status, shared) = response(&router, "recipient", true, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        shared,
        json!({"agent_id":"agt_shared_read","channel_usage":{"active":true,"has_history":true,"first_used_at":null,"last_used_at":null}})
    );
    for private in [
        "owner-a",
        "binding-one",
        "bridge-one",
        "partition-one",
        "created_by",
        "creation_fingerprint",
    ] {
        assert!(!shared.to_string().contains(private));
    }
    assert_eq!(
        response(&router, "recipient", true, "/history").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        response(&router, "recipient", false, "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        response(&router, "recipient", false, "/history").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn channel_usage_history_pages_explicit_attribution_and_preserves_detached_facts() {
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    attach(&fixture.store).await;
    append(
        &fixture.store,
        "use",
        ChannelUsageChange::Use {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::AcceptedCommand,
    )
    .await;
    append(
        &fixture.store,
        "detach",
        ChannelUsageChange::Detach {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::AdministrativeIntervention,
    )
    .await;
    let router =
        super::agent_channel_usage_router(AgentChannelUsageService::new(fixture.store, access));
    let (status, first) = response(&router, "owner-a", true, "/history?limit=2").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["agent_id"], "agt_shared_read");
    assert_eq!(first["next"], "2");
    assert_eq!(first["items"][0]["revision"], 1);
    assert_eq!(first["items"][0]["before"], json!([]));
    assert_eq!(
        first["items"][0]["change"],
        json!({"kind":"attach","binding_id":"binding-one"})
    );
    assert_eq!(first["items"][1]["reason"], "accepted_command");
    let (status, last) = response(&router, "owner-a", false, "/history?limit=2&cursor=2").await;
    assert_eq!(status, StatusCode::OK, "{last}");
    assert!(last.get("next").is_none());
    let event = &last["items"][0];
    assert_eq!(event["revision"], 3);
    assert_eq!(event["detached_at"], "2026-09-15T00:00:00Z");
    assert_eq!(event["reason"], "administrative_intervention");
    assert_eq!(event["after"], json!([]));
    assert_eq!(event["before"][0]["last_used_at"], "2026-09-15T00:00:00Z");
    assert_eq!(event["actor"]["subject"], "owner-a");
    for internal in [
        "fingerprint",
        "operation_id",
        "creation_origin",
        "provisioning_origin",
        "schema_version",
    ] {
        assert!(!last.to_string().contains(internal));
    }
    let (_, summary) = response(&router, "recipient", true, "").await;
    assert_eq!(
        summary["channel_usage"],
        json!({"active":false,"has_history":true,"first_used_at":"2026-09-15T00:00:00Z","last_used_at":"2026-09-15T00:00:00Z"})
    );
}

#[tokio::test]
async fn channel_usage_routes_refuse_invalid_paging_outage_revocation_and_writes() {
    use layer2_core::resource_access::*;
    use tower::ServiceExt;
    let (fixture, access, authority, share) = fixture(Reply::Member).await;
    let router = super::agent_channel_usage_router(AgentChannelUsageService::new(
        fixture.store,
        access.clone(),
    ));
    for suffix in [
        "/history?limit=0",
        "/history?limit=201",
        "/history?limit=garbage",
        "/history?cursor=0",
        "/history?cursor=-1",
        "/history?cursor=01",
        "/history?cursor=1",
        "/history?forged=manual",
    ] {
        let (status, value) = response(&router, "owner-a", true, suffix).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{suffix}: {value}");
        assert_eq!(value["code"], "invalid_query");
    }
    *authority.0.lock().unwrap() = Reply::Outage;
    assert_eq!(
        response(&router, "recipient", true, "").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    *authority.0.lock().unwrap() = Reply::Member;
    access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "channel-usage-revoke".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        response(&router, "recipient", true, "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        response(&router, "recipient", true, "/history").await.0,
        StatusCode::NOT_FOUND
    );
    let mut write = request("owner-a", true, "");
    *write.method_mut() = Method::POST;
    let response = router.clone().oneshot(write).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
}

#[tokio::test]
async fn channel_inventory_routes_filter_explicit_facts_without_relabeling_or_sibling_leaks() {
    use layer2_core::sandbox::AgentProvenanceInventoryService;
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    attach(&fixture.store).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store.clone(),
        access,
    ));
    let get = |query: &str, subject: &str| {
        let mut req = agents_tests::request(
            Method::GET,
            &format!("/api/v1/agents/provenance{query}"),
            subject,
            None,
            None,
        );
        req.extensions_mut().insert(context(subject));
        req.extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
        req
    };
    for scope in ["shared=true", "organization=org"] {
        let (status, body) = agents_tests::json_response(
            &router,
            get(
                &format!("?{scope}&channel_active=true&creation_origin=manual"),
                "recipient",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["agent_id"], "agt_shared_read");
        assert_eq!(body["items"][0]["creation_origin"], "manual");
        assert_eq!(
            body["items"][0]["channel_usage"],
            json!({"active":true,"has_history":true,"first_used_at":null,"last_used_at":null})
        );
        for private in [
            "binding-one",
            "bridge-one",
            "partition-one",
            "owner-a",
            "agt_sibling",
        ] {
            assert!(!body.to_string().contains(private));
        }
    }
    append(
        &fixture.store,
        "detach",
        ChannelUsageChange::Detach {
            binding_id: "binding-one".into(),
        },
        ChannelBindingChangeReason::Detachment,
    )
    .await;
    let (_, body) = agents_tests::json_response(
        &router,
        get(
            "?shared=true&channel_active=false&channel_history=true",
            "recipient",
        ),
    )
    .await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["channel_usage"]["active"], false);
    for query in [
        "?shared=true&channel_active=true",
        "?shared=true&channel_history=false",
        "?shared=true&channel_history=true&creation_origin=channel_bridge",
    ] {
        let (status, body) = agents_tests::json_response(&router, get(query, "recipient")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"], json!([]));
    }
    for query in ["?channel_active=invalid", "?channel_history=1"] {
        let (status, body) = agents_tests::json_response(&router, get(query, "owner-a")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_instance_query");
    }
}
