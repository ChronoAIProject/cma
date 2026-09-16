use super::{
    super::{
        agent_channel_usage_tests::{append, attach},
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{resource_access::*, sandbox::channel_usage::*};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("channel_usage_") {
        return None;
    }
    assert!([
        "channel_usage_empty",
        "channel_usage_owner",
        "channel_usage_shared",
        "channel_usage_shared_detached",
        "channel_usage_history",
        "channel_usage_legacy",
        "channel_usage_revoked",
        "channel_usage_outage",
        "channel_usage_no_credential",
        "channel_usage_anonymous",
        "channel_usage_forbidden",
        "channel_usage_invalid_context",
        "channel_usage_changed",
        "channel_usage_unavailable"
    ]
    .contains(&name));
    let (fixture, access, _, share) = fixture(if name == "channel_usage_outage" {
        Reply::Outage
    } else {
        Reply::Member
    })
    .await;
    if name != "channel_usage_empty" {
        attach(&fixture.store).await;
    }
    if name == "channel_usage_shared_detached" || name == "channel_usage_history" {
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
    }
    if name == "channel_usage_revoked" {
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "usage-revoke".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = if [
        "channel_usage_shared",
        "channel_usage_shared_detached",
        "channel_usage_revoked",
        "channel_usage_outage",
        "channel_usage_no_credential",
    ]
    .contains(&name)
    {
        "recipient"
    } else {
        "owner-a"
    };
    let mut caller = context(subject);
    if name == "channel_usage_forbidden" {
        caller.caller.authorization = layer2_core::Authorization::none();
    }
    let attached = if name == "channel_usage_invalid_context" {
        context("another-principal")
    } else {
        caller.clone()
    };
    let access = if name == "channel_usage_changed" || name == "channel_usage_unavailable" {
        std::sync::Arc::new(ResourceAccessService::new(
            std::sync::Arc::new(super::shared_source_cases::FailingPolicy(
                if name == "channel_usage_changed" {
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
    let mut router = super::super::agent_channel_usage_router(AgentChannelUsageService::new(
        fixture.store.clone(),
        access.clone(),
    ))
    .merge(super::super::agent_provenance_inventory_router(
        layer2_core::sandbox::AgentProvenanceInventoryService::new(fixture.store, access),
    ));
    if name != "channel_usage_legacy" {
        router = router.layer(Extension(attached));
    }
    if name != "channel_usage_no_credential" && name != "channel_usage_legacy" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "channel_usage_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
