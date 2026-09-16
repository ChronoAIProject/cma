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
            AgentProvenanceSnapshot, AgentProvenanceStore, InMemorySandboxStore, SandboxStoreError,
        },
        AgentProvenanceService,
    },
    AgentId,
};
use serde_json::json;
use std::sync::Arc;

const PATH: &str = "/api/v1/agents/agt_shared_read/provenance";
fn request(subject: &str, verified: bool) -> Request<Body> {
    let mut request = agents_tests::request(Method::GET, PATH, subject, None, None);
    if verified {
        request.extensions_mut().insert(context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
    }
    request
}

#[tokio::test]
async fn provenance_owner_and_shared_projection_preserve_unknown_creator() {
    let (fixture, access, _, share) = fixture(Reply::Member).await;
    let service = AgentProvenanceService::new(fixture.store.clone(), access.clone());
    let router = super::agent_provenance_router(service);
    for verified in [false, true] {
        let (status, owner) =
            agents_tests::json_response(&router, request("owner-a", verified)).await;
        assert_eq!(status, StatusCode::OK, "{owner}");
        assert_eq!(owner["creation_origin"], "manual");
        assert_eq!(
            owner["details"],
            json!({"created_by":null,"trigger":null,"channel_bridge":null,"workspace_profile_index":null})
        );
    }
    let (status, shared) = agents_tests::json_response(&router, request("recipient", true)).await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(shared.as_object().unwrap().len(), 3);
    assert_eq!(shared["creation_origin"], "manual");
    assert!(!shared.to_string().contains("owner-a"));
    assert_eq!(
        agents_tests::json_response(&router, request("recipient", false))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
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
    assert_eq!(
        agents_tests::json_response(&router, request("recipient", true))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

struct GatedProvenance {
    inner: Arc<InMemorySandboxStore>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    fail: bool,
}
#[async_trait::async_trait]
impl AgentProvenanceStore for GatedProvenance {
    async fn agent_provenance_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentProvenanceSnapshot, SandboxStoreError> {
        let result = self.inner.agent_provenance_snapshot(id).await;
        self.entered.notify_one();
        self.resume.notified().await;
        if self.fail {
            Err(SandboxStoreError::Corrupt("private-source".into()))
        } else {
            result
        }
    }
}

#[tokio::test]
async fn provenance_fences_buffered_success_and_errors_after_revocation_or_expiry() {
    for expire in [false, true] {
        for fail in [false, true] {
            let (fixture, access, _, share) = fixture(Reply::Member).await;
            let entered = Arc::new(tokio::sync::Notify::new());
            let resume = Arc::new(tokio::sync::Notify::new());
            let service = AgentProvenanceService::new(
                Arc::new(GatedProvenance {
                    inner: fixture.store,
                    entered: entered.clone(),
                    resume: resume.clone(),
                    fail,
                }),
                access.clone(),
            );
            let task = tokio::spawn(async move {
                service
                    .read(
                        &context("recipient"),
                        Some("request-credential"),
                        &AgentId("agt_shared_read".into()),
                    )
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
                .await
                .unwrap();
            if expire {
                tokio::time::pause();
                tokio::time::advance(MAX_MEMBERSHIP_AGE + std::time::Duration::from_secs(1)).await;
            } else {
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
            resume.notify_one();
            let error = task.await.unwrap().unwrap_err();
            if expire {
                tokio::time::resume();
                assert!(matches!(
                    error,
                    ResourceAccessError::Access(AccessError::RevalidationRequired)
                ));
            } else {
                assert!(matches!(
                    error,
                    ResourceAccessError::Store(ResourceAccessStoreError::Conflict)
                ));
            }
        }
    }
}

struct ManagerAuthority;
#[async_trait::async_trait]
impl OrganizationAuthority for ManagerAuthority {
    async fn organization_context(
        &self,
        caller: &layer2_core::Caller,
        _: &str,
        org: &layer2_core::SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut result = context(caller.subject.as_str());
        result.caller = caller.clone();
        let now = std::time::SystemTime::now();
        result.memberships.push(OrganizationMembership::new(
            result.credential.clone(),
            org.clone(),
            OrganizationRole::Admin,
            now,
            now + std::time::Duration::from_secs(1),
        )?);
        Ok(result)
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
async fn provenance_org_management_expiry_withholds_details_despite_operator_read() {
    use layer2_core::sandbox::{store::SandboxStore, AgentStatus};
    let fixture = agents_tests::fixture().await;
    agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_shared_read", AgentStatus::Ready, 0, None),
        Vec::new(),
    )
    .await;
    let root = fixture
        .store
        .get_sandbox(&layer2_core::SandboxId("sbx_route".into()))
        .await
        .unwrap()
        .unwrap();
    fixture
        .store
        .set_resource_policy_for_test(ResourcePolicyRecord {
            sandbox_id: root.id,
            sandbox_created_at: root.created_at.clone(),
            owner: ResourceOwner::Organization {
                subject: root.owner.clone(),
            },
            revision: 1,
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "https://identity.test".into(),
                organization: root.owner,
                admitted_by: context("manager").actor,
                verified_at: root.created_at,
            }),
        });
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(ManagerAuthority),
    ));
    let mut manager = context("manager");
    manager.caller.authorization = layer2_core::Authorization::new([
        layer2_core::Capability::UseOwnSandboxes,
        layer2_core::Capability::ReadAllSandboxes,
    ]);
    let service = AgentProvenanceService::new(fixture.store.clone(), access.clone());
    let id = AgentId("agt_shared_read".into());
    assert!(service
        .read(&manager, Some("request-credential"), &id)
        .await
        .unwrap()
        .details
        .is_some());
    assert!(service
        .read(&manager, None, &id)
        .await
        .unwrap()
        .details
        .is_none());
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let service = AgentProvenanceService::new(
        Arc::new(GatedProvenance {
            inner: fixture.store,
            entered: entered.clone(),
            resume: resume.clone(),
            fail: false,
        }),
        access,
    );
    let task = tokio::spawn(async move {
        service
            .read(&manager, Some("request-credential"), &id)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    resume.notify_one();
    let error = task.await.unwrap().unwrap_err();
    tokio::time::resume();
    assert!(matches!(
        error,
        ResourceAccessError::Access(AccessError::RevalidationRequired)
    ));
}

#[tokio::test]
async fn provenance_refuses_an_agent_identity_replaced_during_the_read() {
    use layer2_core::sandbox::store::AgentStore;
    let (fixture, access, _, _) = fixture(Reply::Member).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let service = AgentProvenanceService::new(
        Arc::new(GatedProvenance {
            inner: fixture.store.clone(),
            entered: entered.clone(),
            resume: resume.clone(),
            fail: false,
        }),
        access,
    );
    let task = tokio::spawn(async move {
        service
            .read(
                &context("owner-a"),
                None,
                &AgentId("agt_shared_read".into()),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let mut agent = fixture
        .store
        .get_agent(&AgentId("agt_shared_read".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    agent.created_at = "2026-09-06T00:00:00Z".into();
    fixture.store.save_agent(revision, agent).await.unwrap();
    resume.notify_one();
    assert!(matches!(
        task.await.unwrap().unwrap_err(),
        ResourceAccessError::Store(ResourceAccessStoreError::Conflict)
    ));
}
