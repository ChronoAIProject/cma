use super::{
    super::{
        agents_tests,
        resource_access_tests::context,
        shared_agent_tests::{fixture_with_origin, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{store::AgentStore, AgentOrigin, AgentProvenanceService},
    AgentId,
};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("provenance_") {
        return None;
    }
    assert!([
        "provenance_owner",
        "provenance_shared",
        "provenance_trigger",
        "provenance_profile",
        "provenance_unknown",
        "provenance_revoked",
        "provenance_no_credential",
        "provenance_anonymous",
        "provenance_created",
        "provenance_outage",
        "provenance_forbidden",
        "provenance_invalid_context",
        "provenance_changed",
        "provenance_unavailable"
    ]
    .contains(&name));
    let origin = match name {
        "provenance_trigger" => AgentOrigin::Trigger {
            trigger_id: "trg-original".into(),
            launch_id: "lch-original".into(),
        },
        "provenance_profile" => AgentOrigin::WorkspaceProfile { index: 17 },
        _ => AgentOrigin::Manual,
    };
    let (fixture, access, _, share) = fixture_with_origin(
        if name == "provenance_outage" {
            Reply::Outage
        } else {
            Reply::Member
        },
        origin.clone(),
    )
    .await;
    let mut id = AgentId("agt_shared_read".into());
    let mut symbols = std::collections::BTreeMap::new();
    if name == "provenance_trigger" || name == "provenance_unknown" {
        let mut agent = fixture.store.get_agent(&id).await.unwrap().unwrap();
        let revision = agent.revision;
        agent.revision += 1;
        agent.creation_key = if name == "provenance_trigger" {
            format!(
                "trigger:{}",
                layer2_core::profiles::content_digest(&origin).unwrap()
            )
        } else {
            "unproven-import".into()
        };
        fixture.store.save_agent(revision, agent).await.unwrap();
    }
    if name == "provenance_created" {
        let mut request = agents_tests::request(
            axum::http::Method::POST,
            "/api/v1/workspaces/wks_route/agents",
            "owner-a",
            Some("provenance-created"),
            Some(serde_json::json!({"agent_profile":{"id":"agp_route","revision":1}})),
        );
        request.extensions_mut().insert(context("owner-a"));
        let (status, body) = agents_tests::json_response(&fixture.router, request).await;
        assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");
        id = AgentId(body["id"].as_str().unwrap().into());
        symbols.insert("created_agent_id".into(), serde_json::json!(id.as_str()));
        symbols.insert("created_agent_at".into(), body["created_at"].clone());
    }
    if name == "provenance_revoked" {
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "provenance-revoke".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = if [
        "provenance_shared",
        "provenance_revoked",
        "provenance_no_credential",
        "provenance_outage",
    ]
    .contains(&name)
    {
        "recipient"
    } else {
        "owner-a"
    };
    let mut caller = context(subject);
    if name == "provenance_forbidden" {
        caller.caller.authorization = layer2_core::Authorization::none();
    }
    let attached = if name == "provenance_invalid_context" {
        context("another-principal")
    } else {
        caller.clone()
    };
    let access = if name == "provenance_changed" || name == "provenance_unavailable" {
        std::sync::Arc::new(ResourceAccessService::new(
            std::sync::Arc::new(super::shared_source_cases::FailingPolicy(
                if name == "provenance_changed" {
                    ResourceAccessStoreError::Conflict
                } else {
                    ResourceAccessStoreError::Unavailable
                },
            )),
            std::sync::Arc::new(super::super::shared_agent_tests::Authority(
                std::sync::Mutex::new(Reply::Member),
            )),
        ))
    } else {
        access
    };
    let mut router =
        super::super::agent_provenance_router(AgentProvenanceService::new(fixture.store, access))
            .layer(Extension(attached));
    if name != "provenance_no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "provenance_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols,
    })
}
