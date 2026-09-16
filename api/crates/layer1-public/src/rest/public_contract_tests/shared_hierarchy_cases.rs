use super::{
    super::{
        hierarchy_router, resource_access_tests::context, shared_agent_tests::Reply,
        shared_hierarchy_tests::fixture,
    },
    shared_source_cases::FailingPolicy,
    Scenario,
};
use axum::Extension;
use layer2_core::{resource_access::*, Authorization};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("shared_hierarchy_")?;
    assert!([
        "whole",
        "narrow",
        "owner",
        "owner_outage",
        "missing",
        "outage",
        "revoked",
        "invalid_context",
        "anonymous",
        "forbidden",
        "no_credential",
        "legacy",
        "changed",
        "unavailable"
    ]
    .contains(&kind));
    let reply = if kind.ends_with("outage") {
        Reply::Outage
    } else if kind == "missing" {
        Reply::Missing
    } else {
        Reply::Member
    };
    let (mut fixture, access, authority, share) = fixture(kind != "narrow", reply).await;
    if kind == "revoked" {
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
    }
    if kind == "changed" || kind == "unavailable" {
        fixture.state.hierarchy =
            fixture
                .state
                .hierarchy
                .with_resource_access(Arc::new(ResourceAccessService::new(
                    Arc::new(FailingPolicy(if kind == "changed" {
                        ResourceAccessStoreError::Conflict
                    } else {
                        ResourceAccessStoreError::Unavailable
                    })),
                    authority,
                )));
        fixture.router = hierarchy_router(fixture.state);
    }
    let subject = if kind.starts_with("owner") {
        "owner-a"
    } else {
        "recipient"
    };
    let mut caller = context(subject);
    if kind == "forbidden" {
        caller.caller.authorization = Authorization::none();
    }
    let attached = if kind == "invalid_context" {
        context("another-principal")
    } else {
        caller.clone()
    };
    let mut router = fixture.router;
    if kind != "legacy" {
        router = router.layer(Extension(attached));
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
        symbols: Default::default(),
    })
}
