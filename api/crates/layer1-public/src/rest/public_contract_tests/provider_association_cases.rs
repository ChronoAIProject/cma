use super::{
    super::{
        agent_provider_associations_tests::{attach, retire, ID},
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*, sandbox::provider_associations::AgentProviderAssociationService,
};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("provider_associations_") {
        return None;
    }
    let (fixture, access, authority, share) = fixture(if name.ends_with("outage") {
        Reply::Outage
    } else {
        Reply::Member
    })
    .await;
    if name != "provider_associations_empty" {
        let conversation = attach(&fixture.store, "binding-private", ID, true).await;
        if name == "provider_associations_retired" {
            retire(&fixture.store, conversation).await;
        }
    }
    if name == "provider_associations_revoked" {
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "association-revoke".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = if [
        "provider_associations_shared",
        "provider_associations_revoked",
        "provider_associations_outage",
    ]
    .contains(&name)
    {
        "recipient"
    } else {
        "owner-a"
    };
    let mut caller = context(subject);
    if name == "provider_associations_forbidden" {
        caller.caller.authorization = layer2_core::Authorization::none();
    }
    let attached = if name == "provider_associations_invalid_context" {
        context("foreign-subject")
    } else {
        caller.clone()
    };
    let access =
        if name == "provider_associations_changed" || name == "provider_associations_unavailable" {
            Arc::new(ResourceAccessService::new(
                Arc::new(super::shared_source_cases::FailingPolicy(
                    if name.ends_with("changed") {
                        ResourceAccessStoreError::Conflict
                    } else {
                        ResourceAccessStoreError::Unavailable
                    },
                )),
                authority,
            ))
        } else {
            access
        };
    let mut router = super::super::agent_provider_associations_router(
        AgentProviderAssociationService::new(fixture.store, access),
    );
    if name != "provider_associations_legacy" {
        router =
            router
                .layer(Extension(attached))
                .layer(Extension(crate::DelegatedCredential::new(
                    "request-credential".into(),
                )));
    }
    Some(Scenario {
        router,
        subject: (name != "provider_associations_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
