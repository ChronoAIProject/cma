use super::*;
use layer2_core::profiles::authoring::*;
use layer2_core::profiles::{ProfileCapacityBounds, ProfilePage};

struct Source(crate::auth::keys::KeyIssuer);

pub(in super::super) fn identity_routes() -> Router {
    Router::new().route(
        "/api/v1/services",
        axum::routing::get(|| async {
            axum::Json(json!([
                {"id":"provider-model", "slug":"chrono-llm-public", "name":"Proof model"},
                {"id":"provider-skills", "slug":"ornn-api", "name":"Proof skills"},
                {"id":"provider-github", "slug":"api-github", "name":"Proof repository"}
            ]))
        }),
    )
}

pub(super) fn router(provider: &str) -> Router {
    layer1_public::rest::profile_authoring_router(
        ProfileAuthoringService::new(
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8000,
                min_memory_mib: 128,
                max_memory_mib: 16384,
            },
            Arc::new(Source(crate::auth::keys::KeyIssuer::new(
                reqwest::Client::new(),
                provider,
                DEPLOYMENT_ID,
            ))),
        ),
        None,
    )
}

#[async_trait::async_trait]
impl ProfileAuthoringSource for Source {
    async fn services(
        &self,
        owner: &SubjectId,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Vec<AuthoringServiceEntry>, AuthoringSourceError> {
        assert_eq!(owner.as_str(), "owner");
        assert_eq!(credential.unwrap().value(), "local-task-owner");
        Ok(self
            .0
            .services_of(credential.unwrap().value())
            .await
            .unwrap()
            .into_iter()
            .map(|service| AuthoringServiceEntry {
                id: service.id,
                catalog_id: Some(service.catalog_id),
                name: service.name,
                description: service.description,
            })
            .collect())
    }
    async fn skills(
        &self,
        _: &SubjectId,
        _: &SkillCatalogQuery,
        _: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError> {
        Err(AuthoringSourceError::Unavailable(
            "No skill catalog is in this bounded external fixture".into(),
        ))
    }
    async fn resolve_repository(
        &self,
        _: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        Err(AuthoringSourceError::Unavailable(
            "Repository preparation uses the native scoped GitHub proxy".into(),
        ))
    }
}
