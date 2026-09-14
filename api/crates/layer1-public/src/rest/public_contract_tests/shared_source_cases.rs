use super::{
    super::{
        resource_access_tests::context,
        shared_agent_tests::{fixture_with_origin, Reply},
        trigger_routes_tests::support_tests::TestApp,
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{store::PageRequest, AgentListQuery, AgentOrigin},
    Authorization, SubjectId,
};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("shared_source_") {
        return None;
    }
    assert!([
        "shared_source_manual",
        "shared_source_workspace",
        "shared_source_trigger",
        "shared_source_owner",
        "shared_source_owner_outage",
        "shared_source_revoked",
        "shared_source_missing",
        "shared_source_outage",
        "shared_source_no_credential",
        "shared_source_invalid_context",
        "shared_source_anonymous",
        "shared_source_forbidden",
        "shared_source_legacy_recipient",
        "shared_source_changed",
        "shared_source_unavailable",
    ]
    .contains(&name));
    let origin = if name.contains("workspace") {
        AgentOrigin::WorkspaceProfile { index: 17 }
    } else if name.contains("trigger") || name.contains("owner") {
        AgentOrigin::Trigger {
            trigger_id: "private-trigger".into(),
            launch_id: "private-launch".into(),
        }
    } else {
        AgentOrigin::Manual
    };
    let reply = if name.ends_with("outage") {
        Reply::Outage
    } else if name.ends_with("missing") {
        Reply::Missing
    } else {
        Reply::Member
    };
    let (fixture, access, authority, share) = fixture_with_origin(reply, origin).await;
    let app = TestApp::new().await;
    if name == "shared_source_revoked" {
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "source-revoke".into(),
                },
            )
            .await
            .unwrap();
    }
    if name == "shared_source_trigger" {
        app.launches.fail(
            "launch",
            layer2_core::trigger_domain::LaunchStoreError::Unavailable("private-dependency".into()),
        );
    }
    let reads = if name == "shared_source_changed" || name == "shared_source_unavailable" {
        fixture
            .state
            .reads
            .with_resource_access(Arc::new(ResourceAccessService::new(
                Arc::new(FailingPolicy(if name.ends_with("changed") {
                    ResourceAccessStoreError::Conflict
                } else {
                    ResourceAccessStoreError::Unavailable
                })),
                authority,
            )))
    } else {
        fixture.state.reads
    };
    let subject = if name.contains("owner") {
        "owner-a"
    } else {
        "recipient"
    };
    let mut caller = context(subject);
    if name == "shared_source_forbidden" {
        caller.caller.authorization = Authorization::none();
    }
    let attached = if name == "shared_source_invalid_context" {
        context("different-principal")
    } else {
        caller.clone()
    };
    let mut router = super::super::agent_source_router(reads, app.source_service);
    if name != "shared_source_legacy_recipient" {
        router = router.layer(Extension(attached));
    }
    if name != "shared_source_no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "shared_source_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}

pub(super) struct FailingPolicy(pub(super) ResourceAccessStoreError);
#[async_trait::async_trait]
impl ResourceAccessStore for FailingPolicy {
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        Err(self.0.clone())
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for FailingPolicy {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        unreachable!()
    }
}
