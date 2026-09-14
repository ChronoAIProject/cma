use super::{
    super::{
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::resource_access::{
    ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStore, ResourceAccessStoreError,
    ResourceSelector, ResourceShare, ResourceShareStore, ResourceTarget, ShareChange, ShareCommand,
    ShareMode, ShareMutation, ShareMutationReceipt,
};
use layer2_core::{AgentId, Authorization, SubjectId};
use std::collections::BTreeMap;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !matches!(
        name,
        "inventory_member"
            | "inventory_two"
            | "inventory_overlap"
            | "inventory_parent"
            | "inventory_owner"
            | "inventory_owner_outage"
            | "inventory_outage"
            | "inventory_missing"
            | "inventory_anonymous"
            | "inventory_forbidden"
            | "inventory_no_credential"
            | "inventory_invalid_context"
            | "inventory_changed"
            | "inventory_store_unavailable"
    ) {
        return None;
    }
    let reply = if name.contains("outage") {
        Reply::Outage
    } else if name == "inventory_missing" {
        Reply::Missing
    } else {
        Reply::Member
    };
    let (mut fixture, access, authority, _) = fixture(reply).await;
    if matches!(name, "inventory_changed" | "inventory_store_unavailable") {
        let error = if name == "inventory_changed" {
            ResourceAccessStoreError::Conflict
        } else {
            ResourceAccessStoreError::Unavailable
        };
        let access = std::sync::Arc::new(ResourceAccessService::new(
            std::sync::Arc::new(FailingInventory {
                inner: fixture.store.clone(),
                error,
            }),
            authority,
        ));
        fixture.state.reads = fixture.state.reads.with_resource_access(access);
        fixture.router = super::super::agent_router(fixture.state.clone());
    }
    if matches!(
        name,
        "inventory_two" | "inventory_overlap" | "inventory_parent"
    ) {
        let selector = if name == "inventory_parent" {
            ResourceSelector::Sandbox {
                sandbox_id: layer2_core::SandboxId("sbx_route".into()),
            }
        } else {
            ResourceSelector::Agent {
                agent_id: AgentId(
                    if name == "inventory_two" {
                        "agt_sibling"
                    } else {
                        "agt_shared_read"
                    }
                    .into(),
                ),
            }
        };
        access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector,
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "inventory-grant".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = if name.contains("owner") {
        "owner-a"
    } else {
        "recipient"
    };
    let mut caller = context(subject);
    if name == "inventory_forbidden" {
        caller.caller.authorization = Authorization::none();
    }
    let attached = if name == "inventory_invalid_context" {
        context("different-principal")
    } else {
        caller.clone()
    };
    let mut router = fixture
        .router
        .merge(super::super::resource_access_router(access))
        .layer(Extension(attached));
    if name != "inventory_no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "inventory_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: BTreeMap::new(),
    })
}

struct FailingInventory {
    inner: std::sync::Arc<layer2_core::sandbox::store::InMemorySandboxStore>,
    error: ResourceAccessStoreError,
}
#[async_trait::async_trait]
impl ResourceAccessStore for FailingInventory {
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &layer2_core::sandbox::AgentListQuery,
        _: &layer2_core::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        Err(self.error.clone())
    }
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.inner.resource_access_snapshot(target).await
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for FailingInventory {
    async fn share_mutation_receipt(
        &self,
        id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.inner.share_mutation_receipt(id).await
    }
    async fn mutate_share(
        &self,
        mutation: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.inner.mutate_share(mutation).await
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.inner.resource_share(id).await
    }
}
