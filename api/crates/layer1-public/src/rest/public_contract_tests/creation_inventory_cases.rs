use super::{
    super::{
        agents_tests,
        resource_access_tests::context,
        shared_agent_tests::{fixture, Reply},
    },
    Scenario,
};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{store::*, AgentListQuery, AgentProvenanceInventoryService, AgentStatus},
    AgentId, Authorization, SandboxId, SubjectId,
};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("creation_inventory_") {
        return None;
    }
    assert!([
        "creation_inventory_owner",
        "creation_inventory_member",
        "creation_inventory_overlap",
        "creation_inventory_sparse",
        "creation_inventory_deleted",
        "creation_inventory_revoked",
        "creation_inventory_no_credential",
        "creation_inventory_anonymous",
        "creation_inventory_outage",
        "creation_inventory_missing",
        "creation_inventory_forbidden",
        "creation_inventory_invalid_context",
        "creation_inventory_changed",
        "creation_inventory_unavailable",
        "creation_inventory_corrupt",
    ]
    .contains(&name));
    let (fixture, access, _, share) = fixture(match name {
        "creation_inventory_outage" => Reply::Outage,
        "creation_inventory_missing" => Reply::Missing,
        _ => Reply::Member,
    })
    .await;
    if name == "creation_inventory_overlap" {
        access
            .change_share(
                &context("owner-a"),
                Some("request-credential"),
                ShareCommand {
                    selector: ResourceSelector::Sandbox {
                        sandbox_id: SandboxId("sbx_route".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "creation-inventory-overlap".into(),
                },
            )
            .await
            .unwrap();
    }
    if name == "creation_inventory_revoked" {
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "creation-inventory-revoke".into(),
                },
            )
            .await
            .unwrap();
    }
    if name == "creation_inventory_sparse" {
        for index in 0..200 {
            agents_tests::insert_agent(
                &fixture.store,
                agents_tests::seeded_agent(
                    &format!("agt_inventory_{index:03}"),
                    AgentStatus::Deleted,
                    0,
                    None,
                ),
                Vec::new(),
            )
            .await;
        }
        let mut agent = fixture
            .store
            .get_agent(&AgentId("agt_sibling".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.revision += 1;
        agent.creation_key = "unproven-legacy-import".into();
        fixture.store.save_agent(revision, agent).await.unwrap();
    }
    if name == "creation_inventory_deleted" {
        let mut agent = fixture
            .store
            .get_agent(&AgentId("agt_shared_read".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.revision += 1;
        agent.status = AgentStatus::Deleted;
        agent.deleted_at = Some(agent.updated_at.clone());
        fixture.store.save_agent(revision, agent).await.unwrap();
    }
    let subject = if matches!(
        name,
        "creation_inventory_owner" | "creation_inventory_sparse"
    ) {
        "owner-a"
    } else {
        "recipient"
    };
    let mut caller = context(subject);
    if name == "creation_inventory_forbidden" {
        caller.caller.authorization = Authorization::none();
    }
    let attached = if name == "creation_inventory_invalid_context" {
        context("another-principal")
    } else {
        caller.clone()
    };
    let store: Arc<dyn AgentProvenanceInventoryStore> = match name {
        "creation_inventory_changed" => Arc::new(FailingInventory(SandboxStoreError::Conflict)),
        "creation_inventory_unavailable" => Arc::new(FailingInventory(
            SandboxStoreError::Unavailable("fixture unavailable".into()),
        )),
        "creation_inventory_corrupt" => Arc::new(FailingInventory(SandboxStoreError::Corrupt(
            "private corrupt source".into(),
        ))),
        _ => fixture.store,
    };
    let mut router = super::super::agent_provenance_inventory_router(
        AgentProvenanceInventoryService::new(store, access),
    )
    .layer(Extension(attached));
    if name != "creation_inventory_no_credential" {
        router = router.layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (name != "creation_inventory_anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}

struct FailingInventory(SandboxStoreError);
#[async_trait::async_trait]
impl AgentProvenanceInventoryStore for FailingInventory {
    async fn scan_agent_provenance_inventory(
        &self,
        _: &ResourceOwner,
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<AgentProvenanceSnapshot>, SandboxStoreError> {
        Err(self.0.clone())
    }
    async fn scan_shared_agent_provenance_inventory(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<AgentProvenanceSnapshot>, SandboxStoreError> {
        Err(self.0.clone())
    }
}
