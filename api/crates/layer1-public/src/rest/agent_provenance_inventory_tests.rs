use super::{
    agents_tests,
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use layer2_core::{
    resource_access::*,
    sandbox::{
        store::{
            AgentProvenanceInventoryStore, AgentProvenanceSnapshot, InMemorySandboxStore,
            PageRequest, SandboxStoreError,
        },
        AgentListQuery, AgentProvenanceInventoryService,
    },
    SubjectId,
};
use std::sync::Arc;

const PATH: &str = "/api/v1/agents/provenance";
fn request(query: &str, subject: &str, verified: bool) -> Request<Body> {
    let mut request =
        agents_tests::request(Method::GET, &format!("{PATH}{query}"), subject, None, None);
    if verified {
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
    }
    request
}

#[tokio::test]
async fn provenance_inventory_has_safe_personal_org_and_filtered_pages_without_parent_leaks() {
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    for verified in [false, true] {
        let (status, body) = agents_tests::json_response(
            &router,
            request("?limit=1&creation_origin=manual", "owner-a", verified),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert!(body["next"].is_string());
        let row = &body["items"][0];
        assert_eq!(row["creation_origin"], "manual");
        assert!(!row.to_string().contains("private"));
        for field in [
            "details",
            "owner",
            "created_by",
            "profile",
            "sandbox_id",
            "worktree_id",
            "workspace_id",
        ] {
            assert!(row.get(field).is_none());
        }
    }
    let (status, body) = agents_tests::json_response(
        &router,
        request(
            "?organization=org&creation_origin=manual",
            "recipient",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["agent_id"], "agt_shared_read");
    assert!(body.get("next").is_none());
    for parent in ["sandbox=sbx_route", "workspace=wks_route"] {
        let (status, _) = agents_tests::json_response(
            &router,
            request(&format!("?organization=org&{parent}"), "recipient", true),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, body) = agents_tests::json_response(
        &router,
        request(
            "?organization=org&creation_origin=channel_bridge",
            "recipient",
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"].as_array().unwrap().is_empty());
    assert!(body.get("next").is_none());
}

#[tokio::test]
async fn provenance_inventory_validates_scope_authority_and_rejects_forged_filters() {
    let (fixture, access, authority, _) = fixture(Reply::Member).await;
    let router = super::agent_provenance_inventory_router(AgentProvenanceInventoryService::new(
        fixture.store,
        access,
    ));
    for query in [
        "?creation_origin=spoofed",
        "?created_by=someone",
        "?limit=0",
        "?limit=201",
        "?cursor=",
        "?organization=%20org",
    ] {
        let (status, body) =
            agents_tests::json_response(&router, request(query, "owner-a", true)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    assert_eq!(
        agents_tests::json_response(&router, request("?organization=org", "recipient", false))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    *authority.0.lock().unwrap() = Reply::Outage;
    assert_eq!(
        agents_tests::json_response(&router, request("?organization=org", "recipient", true))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        agents_tests::json_response(&router, request("", "owner-a", true))
            .await
            .0,
        StatusCode::OK
    );
    *authority.0.lock().unwrap() = Reply::Missing;
    assert_eq!(
        agents_tests::json_response(&router, request("?organization=org", "recipient", true))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

struct GatedInventory {
    inner: Arc<InMemorySandboxStore>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    corrupt: bool,
}
#[async_trait::async_trait]
impl AgentProvenanceInventoryStore for GatedInventory {
    async fn scan_agent_provenance_inventory(
        &self,
        scope: &ResourceOwner,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentProvenanceSnapshot>, SandboxStoreError> {
        let mut result = self
            .inner
            .scan_agent_provenance_inventory(scope, query, page)
            .await?;
        if self.corrupt {
            result[0].creation = Some(
                result[0]
                    .creation_provenance(&result[0].resource.lineage.agent.as_ref().unwrap().id)
                    .unwrap(),
            );
            result[0].creation.as_mut().unwrap().agent_created_at = "corrupt".into();
        }
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(result)
    }
}

#[tokio::test]
async fn provenance_inventory_rechecks_revoked_rows_even_when_filtered_out_or_corrupt() {
    for corrupt in [false, true] {
        let (fixture, access, _, share) = fixture(Reply::Member).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let store = Arc::new(GatedInventory {
            inner: fixture.store,
            entered: entered.clone(),
            resume: resume.clone(),
            corrupt,
        });
        let service = AgentProvenanceInventoryService::new(store, access.clone());
        let pending = tokio::spawn(async move {
            service
                .list(
                    &context("recipient"),
                    Some("request-credential"),
                    &layer2_core::sandbox::AgentProvenanceInventoryQuery {
                        organization: Some(SubjectId::new("org")),
                        creation_origin: Some(
                            layer2_core::sandbox::AgentCreationCategory::ChannelBridge,
                        ),
                        ..Default::default()
                    },
                    &PageRequest::default(),
                )
                .await
        });
        entered.notified().await;
        access
            .change_share(
                &context("owner-a"),
                None,
                ShareCommand {
                    selector: share.selector,
                    change: ShareChange::Revoke { share_id: share.id },
                    expected_share_revision: Some(share.revision),
                    idempotency_key: "provenance-inventory-revoke".into(),
                },
            )
            .await
            .unwrap();
        resume.notify_one();
        assert!(matches!(
            pending.await.unwrap(),
            Err(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            ))
        ));
    }
}

struct ShortAuthority(OrganizationRole);
#[async_trait::async_trait]
impl OrganizationAuthority for ShortAuthority {
    async fn organization_context(
        &self,
        caller: &layer2_core::Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        current.caller = caller.clone();
        let now = std::time::SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            org.clone(),
            self.0,
            now,
            now + std::time::Duration::from_secs(1),
        )?);
        Ok(current)
    }
    async fn organization_choices(
        &self,
        _: &layer2_core::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}

#[tokio::test]
async fn provenance_inventory_expired_scope_refuses_empty_filtered_and_corrupt_pages_with_operator_read(
) {
    use layer2_core::sandbox::{AgentCreationCategory, AgentProvenanceInventoryQuery};
    for (empty, corrupt) in [(true, false), (false, false), (false, true)] {
        let (fixture, _, _, _) = fixture(Reply::Member).await;
        let access = Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(ShortAuthority(OrganizationRole::Admin)),
        ));
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let service = AgentProvenanceInventoryService::new(
            Arc::new(GatedInventory {
                inner: fixture.store,
                entered: entered.clone(),
                resume: resume.clone(),
                corrupt,
            }),
            access,
        );
        let mut caller = context("recipient");
        caller.caller.authorization = layer2_core::Authorization::new([
            layer2_core::Capability::UseOwnSandboxes,
            layer2_core::Capability::ReadAllSandboxes,
        ]);
        let pending = tokio::spawn(async move {
            service
                .list(
                    &caller,
                    Some("request-credential"),
                    &AgentProvenanceInventoryQuery {
                        organization: Some(SubjectId::new(if empty { "empty-org" } else { "org" })),
                        creation_origin: Some(AgentCreationCategory::ChannelBridge),
                        ..Default::default()
                    },
                    &PageRequest::default(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(2)).await;
        resume.notify_one();
        let error = pending.await.unwrap().unwrap_err();
        tokio::time::resume();
        assert!(matches!(
            error,
            ResourceAccessError::Access(AccessError::RevalidationRequired)
        ));
    }
}

#[tokio::test]
async fn provenance_inventory_org_ownership_is_explicit_for_every_read_role_and_same_subject_personal_scope(
) {
    use layer2_core::sandbox::{store::SandboxStore, AgentProvenanceInventoryQuery};
    for role in [
        OrganizationRole::Viewer,
        OrganizationRole::Member,
        OrganizationRole::Admin,
    ] {
        let fixture = agents_tests::fixture().await;
        for id in ["agt_org_first", "agt_org_second"] {
            agents_tests::insert_agent(
                &fixture.store,
                agents_tests::seeded_agent(id, layer2_core::sandbox::AgentStatus::Ready, 0, None),
                Vec::new(),
            )
            .await;
        }
        let root = fixture
            .store
            .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
            .await
            .unwrap()
            .unwrap();
        fixture
            .store
            .set_resource_policy_for_test(ResourcePolicyRecord {
                sandbox_id: root.id.clone(),
                sandbox_created_at: root.created_at.clone(),
                owner: ResourceOwner::Organization {
                    subject: root.owner.clone(),
                },
                revision: 1,
                organization_evidence: Some(OrganizationOwnershipEvidence {
                    issuer: "https://identity.test".into(),
                    organization: root.owner.clone(),
                    admitted_by: context("manager").actor,
                    verified_at: root.created_at,
                }),
            });
        let access = Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(ShortAuthority(role)),
        ));
        let service = AgentProvenanceInventoryService::new(fixture.store, access);
        let org = service
            .list(
                &context("reader"),
                Some("request-credential"),
                &AgentProvenanceInventoryQuery {
                    organization: Some(root.owner.clone()),
                    agents: AgentListQuery {
                        sandbox_id: Some(root.id),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                &PageRequest::default(),
            )
            .await
            .unwrap();
        assert_eq!(org.items.len(), 2);
        assert!(org.next.is_none());
        assert!(service
            .list(
                &context(root.owner.as_str()),
                None,
                &AgentProvenanceInventoryQuery::default(),
                &PageRequest::default()
            )
            .await
            .unwrap()
            .items
            .is_empty());
    }
}
