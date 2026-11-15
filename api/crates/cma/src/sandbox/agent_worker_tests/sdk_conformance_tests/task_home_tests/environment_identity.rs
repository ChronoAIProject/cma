use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::OwnerTokenSource;
use axum::{
    routing::{delete, get, post},
    Json,
};

pub(super) struct LocalIdentity;

pub(super) struct RepositoryConnection(pub(super) std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl layer2_core::sandbox::SandboxCredentialPrerequisite for RepositoryConnection {
    async fn check(
        &self,
        _: &SubjectId,
    ) -> Result<(), layer2_core::sandbox::SandboxCredentialPrerequisiteError> {
        Ok(())
    }

    async fn check_services(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<(), layer2_core::sandbox::SandboxCredentialPrerequisiteError> {
        if services
            .iter()
            .any(|service| service.as_str() == "api-github")
            && !self.0.load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(
                layer2_core::sandbox::SandboxCredentialPrerequisiteError::ConnectionsRequired {
                    services: vec![ServiceId::new("api-github")],
                },
            )
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl OwnerTokenSource for LocalIdentity {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(Some(SecretValue::new("local-task-owner".into())))
    }
}

pub(super) async fn identity_provider() -> (String, tokio::task::JoinHandle<()>) {
    let routes = Router::new()
        .merge(super::integrated_tests::catalog::identity_routes())
        .route(
            "/api/v1/user-services",
            get(|| async {
                Json(json!([
                    {"id":"provider-model", "slug":"chrono-llm-public", "is_active":true},
                    {"id":"provider-skills", "slug":"ornn-api", "is_active":true},
                    {"id":"provider-github", "slug":"api-github", "is_active":true}
                ]))
            }),
        )
        .route(
            "/api/v1/api-keys/scope-plan",
            post(|Json(body): Json<Value>| async move {
                Json(crate::auth::keys::scope_plan_fixture("owner", &body, &[]))
            }),
        )
        .route(
            "/api/v1/api-keys",
            post(|Json(body): Json<Value>| async move {
                Json(json!({
                    "id":"local-task-key", "full_key":"local-task-runtime-credential",
                    "scopes":"proxy read", "state_version":1,
                    "allowed_service_ids":body["allowed_service_ids"],
                    "allowed_node_ids":body["allowed_node_ids"],
                    "allow_all_services":false, "allow_all_nodes":false
                }))
            }),
        )
        .route(
            "/api/v1/api-keys/{id}",
            delete(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/proxy", get(|| async { StatusCode::NO_CONTENT }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    (endpoint, task)
}
