use super::{execution::*, fixtures::*};
use async_trait::async_trait;
use futures_util::StreamExt;
use layer2_core::{
    resource_access::*,
    sandbox::{agent_io::AgentCommandWorkerStore, store::*, AgentListQuery, OwnerSandboxQuery},
    SubjectId,
};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tower::ServiceExt;

struct InterruptedAccess {
    inner: Arc<InMemorySandboxStore>,
    interrupted: Arc<AtomicBool>,
    unavailable: bool,
}
#[async_trait]
impl ResourceShareStore for InterruptedAccess {
    async fn share_mutation_receipt(
        &self,
        id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.inner.share_mutation_receipt(id).await
    }
    async fn mutate_share(
        &self,
        request: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.inner.mutate_share(request).await
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.inner.resource_share(id).await
    }
}
#[async_trait]
impl ResourceAccessStore for InterruptedAccess {
    async fn organization_sandboxes(
        &self,
        subject: &SubjectId,
        query: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_sandboxes(subject, query).await
    }
    async fn shared_sandboxes(
        &self,
        subject: &SubjectId,
        organizations: &[SubjectId],
        query: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .shared_sandboxes(subject, organizations, query)
            .await
    }
    async fn organization_agents(
        &self,
        subject: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_agents(subject, query, page).await
    }
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        if self.interrupted.load(Ordering::SeqCst) {
            Err(if self.unavailable {
                ResourceAccessStoreError::Unavailable
            } else {
                ResourceAccessStoreError::Conflict
            })
        } else {
            self.inner.resource_access_snapshot(target).await
        }
    }
}

#[tokio::test]
async fn buffered_legacy_frames_stop_when_fresh_authorization_or_read_evidence_is_lost() {
    for path in [AGUI, RESPONSES] {
        for unavailable in [false, true] {
            let mut f = fixture(false).await;
            let interrupted = Arc::new(AtomicBool::new(false));
            f.state.reads =
                f.state
                    .reads
                    .with_resource_access(Arc::new(ResourceAccessService::new(
                        Arc::new(InterruptedAccess {
                            inner: f.store.clone(),
                            interrupted: interrupted.clone(),
                            unavailable,
                        }),
                        Arc::new(crate::conversation::command_test_support::Authority),
                    )));
            let router = crate::agui::router(f.state.clone())
                .merge(crate::responses::router(f.state.clone()));
            let input = if path == AGUI {
                json!({"threadId":ID,"messages":[{"role":"user","content":"Private input"}]})
            } else {
                json!({"conversation":ID,"input":"Private input","stream":true})
            };
            let task = tokio::spawn(router.oneshot(request(path, Some("buffered"), input)));
            let original = admitted(&f).await.remove(0);
            execute(&f).await;
            finish(&f, "Private buffered answer").await;
            let response = tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let mut stream = response.into_body().into_data_stream();
            let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(std::str::from_utf8(&first)
                .unwrap()
                .contains(if path == AGUI {
                    "RUN_STARTED"
                } else {
                    "response.created"
                }));
            interrupted.store(true, Ordering::SeqCst);
            assert!(tokio::time::timeout(Duration::from_secs(2), stream.next()).await.unwrap().is_none(),
                "no buffered private frame or invented failure may follow lost authorization evidence");
            let retained = f
                .store
                .reconcile_command(&original.receipt.envelope.command_id)
                .await
                .unwrap();
            assert_eq!(
                retained.status,
                layer2_core::sandbox::agent_io::AgentResponseStatus::Completed
            );
            assert!(retained.work.failure.is_none());
            assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
        }
    }
}
