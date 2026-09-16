use super::{
    agents_tests::{self, json_response},
    resource_access_tests::context,
    shared_agent_tests::{fixture_with_origin, Reply},
    trigger_routes_tests::support_tests::TestApp,
};
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use layer2_core::{
    resource_access::*,
    sandbox::{AgentOrigin, AgentReadError, AgentSourceReadError},
    AgentId,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const PATH: &str = "/api/v1/agents/agt_shared_read/source";

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
async fn shared_source_categories_preserve_owner_source_and_never_read_private_launches() {
    use layer2_core::sandbox::store::AgentStore;
    for (origin, kind) in [
        (AgentOrigin::Manual, "manual"),
        (
            AgentOrigin::WorkspaceProfile { index: 17 },
            "workspace_profile",
        ),
        (
            AgentOrigin::Trigger {
                trigger_id: "private-trigger".into(),
                launch_id: "private-launch".into(),
            },
            "trigger",
        ),
    ] {
        let app = TestApp::new().await;
        let (fixture, _, _, _) = fixture_with_origin(Reply::Member, origin.clone()).await;
        let id = AgentId("agt_shared_read".into());
        let before = fixture.store.get_agent(&id).await.unwrap().unwrap();
        let router = super::agent_source_router(fixture.state.reads.clone(), app.source_service);
        let (legacy_status, legacy) = json_response(&router, request("owner-a", false)).await;
        let (status, personal) = json_response(&router, request("owner-a", true)).await;
        assert_eq!(legacy_status, StatusCode::OK);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(personal, legacy);
        if kind == "trigger" {
            assert_eq!(personal["origin"]["launch_id"], "private-launch");
        }
        app.launches.fail(
            "launch",
            layer2_core::trigger_domain::LaunchStoreError::Unavailable("private-dependency".into()),
        );
        let (status, shared) = json_response(&router, request("recipient", true)).await;
        assert_eq!(status, StatusCode::OK, "{shared}");
        assert_eq!(
            shared,
            json!({"access":"shared","agent_id":"agt_shared_read","origin":{"type":kind}})
        );
        if kind == "trigger" {
            let (status, failure) = json_response(&router, request("owner-a", true)).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(failure["code"], "trigger_service_unavailable");
        }
        assert_eq!(fixture.store.get_agent(&id).await.unwrap().unwrap(), before);
    }
}

#[tokio::test]
async fn buffered_shared_source_refuses_revoked_or_expired_access() {
    for expire in [false, true] {
        let app = TestApp::new().await;
        let (fixture, access, authority, share) =
            fixture_with_origin(Reply::Member, AgentOrigin::Manual).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let reads = fixture
            .state
            .reads
            .with_resource_access(Arc::new(ResourceAccessService::new(
                Arc::new(GatedPolicy {
                    inner: fixture.store.clone(),
                    calls: AtomicUsize::new(0),
                    entered: entered.clone(),
                    resume: resume.clone(),
                }),
                authority,
            )));
        let task = tokio::spawn(async move {
            reads
                .source_authorized(
                    &context("recipient"),
                    Some("request-credential"),
                    &AgentId("agt_shared_read".into()),
                    &app.source_service,
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
                        idempotency_key: "source-revoke".into(),
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
                AgentSourceReadError::Agent(AgentReadError::Access(ResourceAccessError::Access(
                    AccessError::RevalidationRequired
                )))
            ));
        } else {
            assert!(matches!(
                error,
                AgentSourceReadError::Agent(AgentReadError::Access(ResourceAccessError::Store(
                    ResourceAccessStoreError::Conflict
                )))
            ));
        }
    }
}

struct GatedPolicy {
    inner: Arc<layer2_core::sandbox::store::InMemorySandboxStore>,
    calls: AtomicUsize,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}
#[async_trait::async_trait]
impl ResourceAccessStore for GatedPolicy {
    async fn organization_sandboxes(
        &self,
        _organization: &layer2_core::SubjectId,
        _query: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }

    async fn shared_sandboxes(
        &self,
        _recipient: &layer2_core::SubjectId,
        _organizations: &[layer2_core::SubjectId],
        _query: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }

    async fn organization_agents(
        &self,
        _: &layer2_core::SubjectId,
        _: &layer2_core::sandbox::AgentListQuery,
        _: &layer2_core::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 2 {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.inner.resource_access_snapshot(target).await
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for GatedPolicy {
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
