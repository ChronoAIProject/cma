use super::{
    super::{
        agents_tests, hierarchy_router, resource_access_tests::context, shared_agent_tests::Reply,
        shared_hierarchy_tests::fixture,
    },
    shared_source_cases::FailingPolicy,
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*, sandbox::store::SandboxStore, Authorization, SandboxId, SubjectId,
};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("sandbox_inventory_")?;
    assert!([
        "whole",
        "narrow",
        "two",
        "overlap",
        "owner",
        "owner_outage",
        "outage",
        "missing",
        "revoked",
        "invalid_context",
        "anonymous",
        "forbidden",
        "no_credential",
        "legacy",
        "changed",
        "unavailable",
        "org_owned"
    ]
    .contains(&kind));
    let (mut fixture, access, authority, share) =
        fixture(!["narrow", "org_owned"].contains(&kind), Reply::Member).await;
    for id in ["a_private", "z_other"] {
        let mut sandbox = agents_tests::sandbox(id, "owner-a");
        sandbox.name = id.into();
        fixture.store.create_sandbox(sandbox).await.unwrap();
    }
    if kind == "two" || kind == "overlap" {
        access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: SandboxId(
                            if kind == "two" {
                                "z_other"
                            } else {
                                "sbx_route"
                            }
                            .into(),
                        ),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "another-grant".into(),
                },
            )
            .await
            .unwrap();
    }
    if kind == "revoked" || kind == "org_owned" {
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
    if kind == "org_owned" {
        let root = fixture
            .store
            .get_sandbox(&SandboxId("sbx_route".into()))
            .await
            .unwrap()
            .unwrap();
        fixture
            .store
            .set_resource_policy_for_test(ResourcePolicyRecord {
                sandbox_id: root.id,
                sandbox_created_at: root.created_at,
                owner: ResourceOwner::Organization {
                    subject: root.owner.clone(),
                },
                revision: 3,
                organization_evidence: Some(OrganizationOwnershipEvidence {
                    issuer: "https://identity.test".into(),
                    organization: root.owner,
                    admitted_by: ResourceActor::Human {
                        subject: SubjectId::new("fixture-admin"),
                    },
                    verified_at: "2026-09-05T00:00:00Z".into(),
                }),
            });
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
                    authority.clone(),
                )));
        fixture.router = hierarchy_router(fixture.state);
    }
    *authority.0.lock().unwrap() = if kind.ends_with("outage") {
        Reply::Outage
    } else if kind == "missing" {
        Reply::Missing
    } else {
        Reply::Member
    };
    let subject = if kind.starts_with("owner") || kind == "org_owned" {
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
