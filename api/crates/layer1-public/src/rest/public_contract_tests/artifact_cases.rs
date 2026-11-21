use super::{
    super::{
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{resource_access::*, sandbox::agent_artifacts::*, AgentId};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("artifact_")?;
    assert!([
        "owner",
        "member",
        "missing",
        "outage",
        "revoked",
        "anonymous",
        "no_credential",
        "legacy",
        "invalid_context",
        "capacity"
    ]
    .contains(&kind));
    let (fixture, access, authority, share) = fixture(Reply::Member).await;
    let service = AgentArtifactService::new(fixture.store.clone(), access.clone());
    let owner = context("owner-a");
    let metadata = service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &AgentId("agt_shared_read".into()),
            "fixture",
            PublishAgentArtifact {
                name: "report.txt".into(),
                media_type: "text/plain".into(),
                content_base64: "YXBwcm92ZWQK".into(),
            },
        )
        .await
        .unwrap();
    if kind == "capacity" {
        for index in 1..256 {
            service
                .publish(
                    &owner.caller,
                    Some(&owner),
                    None,
                    &AgentId("agt_shared_read".into()),
                    &format!("capacity-{index}"),
                    PublishAgentArtifact {
                        name: format!("item-{index}.txt"),
                        media_type: "text/plain".into(),
                        content_base64: "YQ==".into(),
                    },
                )
                .await
                .unwrap();
        }
    }
    let mut symbols = BTreeMap::new();
    if ["owner", "member", "legacy"].contains(&kind) {
        symbols.insert("artifact_id".into(), Value::String(metadata.id));
        symbols.insert("created_at".into(), Value::String(metadata.created_at));
    }
    match kind {
        "missing" => *authority.0.lock().unwrap() = Reply::Missing,
        "outage" => *authority.0.lock().unwrap() = Reply::Outage,
        "revoked" => {
            access
                .change_share(
                    &owner,
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
        }
        _ => (),
    }
    let subject = if ["owner", "legacy", "capacity"].contains(&kind) {
        "owner-a"
    } else {
        "recipient"
    };
    let caller = context(subject);
    let mut router = super::super::agent_artifacts_router(service);
    if kind != "legacy" {
        router = router.layer(Extension(if kind == "invalid_context" {
            context("owner-a")
        } else {
            caller.clone()
        }));
    }
    if kind != "no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (kind != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols,
    })
}
