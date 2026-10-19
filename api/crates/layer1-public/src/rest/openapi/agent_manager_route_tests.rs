use super::*;
use crate::rest;
use layer2_core::sandbox::{agent_artifacts::AgentArtifactService, agent_attachments::*, Sandbox};
use std::sync::Arc;

struct NoProviderCalls;
#[async_trait::async_trait]
impl AttachmentWriter for NoProviderCalls {
    async fn write(
        &self,
        _: &Sandbox,
        _: &AgentAttachment,
        _: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        panic!("anonymous discovery probe cannot upload")
    }
}
#[async_trait::async_trait]
impl rest::AgentModelCatalog for NoProviderCalls {
    async fn models(
        &self,
        _: &Sandbox,
    ) -> Result<Vec<rest::AgentModelView>, rest::AgentModelCatalogError> {
        panic!("anonymous discovery probe cannot read provider models")
    }
}
#[async_trait::async_trait]
impl rest::AgentModelRuntimeSupport for NoProviderCalls {
    async fn supported(&self, _: &Sandbox) -> bool {
        panic!("anonymous discovery probe cannot inspect runtime support")
    }
}

/// Probe actual composed routers before authentication. A wrong method or
/// absent path would return 405/404, rather than reaching an auth boundary.
#[tokio::test]
async fn every_selected_method_and_path_exists_in_the_actual_routers() {
    let agents = rest::agents_tests::fixture().await;
    let hierarchy = rest::instances_tests::fixture(3);
    let events = crate::agent_v2::fixtures::fixture(false, false).await;
    let access = rest::agent_access_requests_tests::app().0;
    let router = agents
        .router
        .merge(hierarchy.router)
        .merge(events.router)
        .merge(access)
        .merge(rest::agent_models_router(
            agents.state.reads.clone(),
            Arc::new(NoProviderCalls),
            Arc::new(NoProviderCalls),
        ))
        .merge(rest::agent_attachments_router(
            AgentAttachmentService::with_writer(events.access.clone(), Arc::new(NoProviderCalls)),
        ))
        .merge(rest::agent_artifacts_router(AgentArtifactService::new(
            events.store,
            events.access,
        )));
    for (path, methods) in OPERATIONS {
        let concrete = path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "fixture"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        for method in *methods {
            let request = Request::builder()
                .method(method.to_ascii_uppercase().as_str())
                .uri(&concrete)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            assert!(
                ![StatusCode::NOT_FOUND, StatusCode::METHOD_NOT_ALLOWED]
                    .contains(&response.status()),
                "missing live route {method} {path}: {}",
                response.status()
            );
            assert!(
                response.status() == StatusCode::UNAUTHORIZED
                    || (*path == "/api/v2" && response.status() == StatusCode::OK),
                "unexpected unauthenticated outcome {method} {path}: {}",
                response.status()
            );
        }
    }
}
