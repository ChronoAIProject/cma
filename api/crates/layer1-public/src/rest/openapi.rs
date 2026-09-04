//! OpenAPI generation for the currently composed REST boundary.

use utoipa::{openapi, OpenApi};

use super::openapi_shapes::ConformanceDocument;
use super::problems::ProblemDocument;
use super::profiles_openapi_shapes::*;

#[derive(OpenApi)]
#[openapi(
    paths(
        super::conformance,
        super::profiles_agent::create,
        super::profiles_agent::list,
        super::profiles_agent::read,
        super::profiles_agent::update,
        super::profiles_agent::publish,
        super::profiles_agent::list_revisions,
        super::profiles_agent::read_revision,
        super::profiles_agent::set_visibility,
        super::profiles_agent::retire,
        super::profiles_workspace::create,
        super::profiles_workspace::list,
        super::profiles_workspace::read,
        super::profiles_workspace::update,
        super::profiles_workspace::publish,
        super::profiles_workspace::list_revisions,
        super::profiles_workspace::read_revision,
        super::profiles_workspace::set_visibility,
        super::profiles_workspace::retire,
        super::profiles_sandbox::create,
        super::profiles_sandbox::list,
        super::profiles_sandbox::read,
        super::profiles_sandbox::update,
        super::profiles_sandbox::publish,
        super::profiles_sandbox::list_revisions,
        super::profiles_sandbox::read_revision,
        super::profiles_sandbox::set_visibility,
        super::profiles_sandbox::retire,
        super::profiles_marketplace::list,
        super::profiles_marketplace::read,
        super::profiles_marketplace::fork,
    ),
    components(schemas(
        ConformanceDocument,
        ProblemDocument,
        ProfileVisibilityValue,
        ThinkingEffortValue,
        PermissionLevelValue,
        AgentProviderValue,
        ModelAccessValue,
        MarketplaceKindValue,
        AgentProfileReference,
        WorkspaceProfileReference,
        SandboxProfileReference,
        SkillReference,
        SkillSourceValue,
        AgentProfileContentBody,
        WorkspaceLocationBody,
        WorkspaceProfileContentBody,
        EnvironmentVariableBody,
        SandboxProfileContentBody,
        CreateAgentProfileRequest,
        UpdateAgentProfileRequest,
        CreateWorkspaceProfileRequest,
        UpdateWorkspaceProfileRequest,
        CreateSandboxProfileRequest,
        UpdateSandboxProfileRequest,
        PublishProfileRequest,
        SetProfileVisibilityRequest,
        ForkProfileRequest,
        ProfileRevisionSummary,
        AgentProfileRevisionView,
        WorkspaceProfileRevisionView,
        SandboxProfileRevisionView,
        AgentProfileView,
        WorkspaceProfileView,
        SandboxProfileView,
        AgentProfilePageView,
        WorkspaceProfilePageView,
        SandboxProfilePageView,
        AgentProfileRevisionPageView,
        WorkspaceProfileRevisionPageView,
        SandboxProfileRevisionPageView,
        MarketplaceSummaryView,
        MarketplacePageView,
        ResolvedProfileNameView,
        SandboxProfileTrustView,
        MarketplaceEntryView,
        ForkedProfileView,
    )),
    tags(
        (name = "discovery", description = "Anonymous deployment discovery"),
        (name = "agent profiles", description = "Owned Agent Profile drafts and immutable revisions"),
        (name = "workspace profiles", description = "Owned Workspace Profile drafts and immutable revisions"),
        (name = "sandbox profiles", description = "Owned Sandbox Profile drafts and immutable revisions"),
        (name = "profile marketplace", description = "Published profiles available to authenticated callers"),
    )
)]
struct ApiDoc;

pub fn document() -> utoipa::openapi::OpenApi {
    let mut document = ApiDoc::openapi();
    let components = document
        .components
        .get_or_insert_with(openapi::Components::new);
    components.add_security_scheme(
        "bearerAuth",
        openapi::security::SecurityScheme::Http(
            openapi::security::HttpBuilder::new()
                .scheme(openapi::security::HttpAuthScheme::Bearer)
                .build(),
        ),
    );
    components.add_security_scheme(
        "sessionCookie",
        openapi::security::SecurityScheme::ApiKey(openapi::security::ApiKey::Cookie(
            openapi::security::ApiKeyValue::with_description(
                "cma_session",
                "CMA browser session cookie",
            ),
        )),
    );
    for (path, item) in &mut document.paths.paths {
        for (method, operation) in [
            ("get", item.get.as_mut()),
            ("put", item.put.as_mut()),
            ("post", item.post.as_mut()),
            ("delete", item.delete.as_mut()),
            ("patch", item.patch.as_mut()),
        ]
        .into_iter()
        .filter_map(|(method, operation)| operation.map(|operation| (method, operation)))
        {
            operation.operation_id = Some(operation_id(method, path));
            operation.security = Some(if path == "/api/v1/conformance" {
                Vec::new()
            } else {
                vec![
                    openapi::security::SecurityRequirement::new(
                        "bearerAuth",
                        std::iter::empty::<String>(),
                    ),
                    openapi::security::SecurityRequirement::new(
                        "sessionCookie",
                        std::iter::empty::<String>(),
                    ),
                ]
            });
        }
    }
    document
}

fn operation_id(method: &str, path: &str) -> String {
    let mut id = method.to_owned();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        id.push('_');
        if segment.starts_with('{') && segment.ends_with('}') {
            id.push_str("by_");
        }
        let mut previous_was_separator = false;
        for character in segment.trim_matches(['{', '}']).chars() {
            if character.is_ascii_alphanumeric() || character == '_' {
                id.push(character.to_ascii_lowercase());
                previous_was_separator = false;
            } else if !previous_was_separator {
                id.push('_');
                previous_was_separator = true;
            }
        }
        if id.ends_with('_') {
            id.pop();
        }
    }
    id
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_profile_and_marketplace_route_is_documented_and_authenticated() {
        let document = document();
        assert_eq!(document.paths.paths.len(), 22);
        let mut operation_ids = HashSet::new();
        for (path, item) in &document.paths.paths {
            for operation in [item.get.as_ref(), item.put.as_ref(), item.post.as_ref()]
                .into_iter()
                .flatten()
            {
                assert!(operation_ids.insert(operation.operation_id.as_deref().unwrap()));
                if path == "/api/v1/conformance" {
                    assert!(operation.security.as_ref().is_some_and(Vec::is_empty));
                } else {
                    assert_eq!(operation.security.as_ref().map(Vec::len), Some(2));
                }
            }
        }
        for path in [
            "/api/v1/agent-profiles",
            "/api/v1/workspace-profiles/{id}/revisions/{revision}",
            "/api/v1/sandbox-profiles/{id}/visibility",
            "/api/v1/marketplace/{kind}/{id}/fork",
        ] {
            assert!(document.paths.paths.contains_key(path), "missing {path}");
        }
        assert_eq!(
            operation_id("post", "/api/v1/marketplace/{kind}/{id}/fork"),
            "post_api_v1_marketplace_by_kind_by_id_fork"
        );
    }
}
