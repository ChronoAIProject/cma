//! REST projection of environment profiles.
//!
//! A plain resource collection: create, read, replace, remove, plus the
//! catalogue a client picks skills from. Every handler is a thin translation
//! onto one core call — the ownership rule, the validation and the
//! non-revocable baseline all live in Layer 2, so no interface can apply a
//! different version of them.
//!
//! Variable values travel in and out because a client that could not read back
//! what it stored could not offer an edit form. They are never logged here,
//! and the core's own `Debug` keeps them out of a log elsewhere.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use layer2_core::{
    CatalogSkill, EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService,
    EnvironmentProfileSpec, EnvironmentVariable, SkillId, SoftwarePackage,
};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::rest::Problem;

/// A variable as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize)]
pub struct VariableBody {
    /// Variable name.
    pub key: String,
    /// Value. Opaque to every layer that carries it.
    pub value: String,
}

/// A package as this interface reads and writes it.
#[derive(Debug, Deserialize, Serialize)]
pub struct PackageBody {
    /// Package name.
    pub name: String,
    /// Version to pin, when the caller cares which one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Request body for creating or replacing a profile.
#[derive(Debug, Deserialize)]
pub struct ProfileRequest {
    /// Name the owner recognises it by.
    pub name: String,
    /// Variables to set in the session.
    #[serde(default)]
    pub variables: Vec<VariableBody>,
    /// Software the session should carry.
    #[serde(default)]
    pub packages: Vec<PackageBody>,
    /// Skill identifiers to load.
    #[serde(default)]
    pub skills: Vec<String>,
}

impl From<ProfileRequest> for EnvironmentProfileSpec {
    fn from(body: ProfileRequest) -> Self {
        Self {
            name: body.name,
            variables: body
                .variables
                .into_iter()
                .map(|v| EnvironmentVariable::new(v.key, v.value))
                .collect(),
            packages: body
                .packages
                .into_iter()
                .map(|p| SoftwarePackage {
                    name: p.name,
                    version: p.version,
                })
                .collect(),
            skills: body.skills.into_iter().map(SkillId::new).collect(),
        }
    }
}

/// A profile as this interface renders it.
#[derive(Debug, Serialize)]
pub struct ProfileView {
    id: String,
    name: String,
    variables: Vec<VariableBody>,
    packages: Vec<PackageBody>,
    skills: Vec<String>,
    /// Which entries the platform mandates, so a client can render them as
    /// fixed rather than guessing — and so the rule is visible rather than
    /// only enforced when the client gets it wrong.
    mandatory: MandatoryView,
    created_at: String,
    updated_at: String,
}

/// The parts of a profile a caller may not remove.
#[derive(Debug, Default, Serialize)]
pub struct MandatoryView {
    packages: Vec<String>,
    skills: Vec<String>,
}

impl MandatoryView {
    fn of(baseline: &layer2_core::Baseline) -> Self {
        Self {
            packages: baseline.packages().iter().map(|p| p.name.clone()).collect(),
            skills: baseline.skills().iter().map(|s| s.0.clone()).collect(),
        }
    }
}

impl ProfileView {
    fn render(profile: EnvironmentProfile, service: &EnvironmentProfileService) -> Self {
        Self {
            id: profile.id.0,
            name: profile.name,
            variables: profile
                .variables
                .into_iter()
                .map(|v| VariableBody {
                    key: v.key,
                    value: v.value,
                })
                .collect(),
            packages: profile
                .packages
                .into_iter()
                .map(|p| PackageBody {
                    name: p.name,
                    version: p.version,
                })
                .collect(),
            skills: profile.skills.into_iter().map(|s| s.0).collect(),
            mandatory: MandatoryView::of(service.baseline()),
            created_at: profile.created_at,
            updated_at: profile.updated_at,
        }
    }
}

/// What a client needs to render the profile editor before it has a profile.
///
/// The mandated entries ride along with the catalogue rather than only with a
/// stored profile: a client authoring its *first* profile has no profile to
/// read them from, and would otherwise offer a form that quietly acquires
/// entries the moment it is saved.
#[derive(Debug, Serialize)]
pub struct SkillCatalogView {
    skills: Vec<SkillView>,
    mandatory: MandatoryView,
}

/// A selectable skill as this interface renders it.
#[derive(Debug, Serialize)]
pub struct SkillView {
    id: String,
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    access: Option<String>,
}

impl From<CatalogSkill> for SkillView {
    fn from(s: CatalogSkill) -> Self {
        Self {
            id: s.id.0,
            name: s.name,
            description: s.description,
            access: s.access,
        }
    }
}

/// Routes this resource contributes.
///
/// The catalogue sits under the collection because it exists only to be picked
/// from when authoring one; it is a static segment, which a router matches
/// ahead of the identifier pattern beside it.
pub fn router(service: Arc<EnvironmentProfileService>) -> Router {
    Router::new()
        .route("/api/v1/environment-profiles", get(list).post(create))
        .route("/api/v1/environment-profiles/skills", get(skills))
        .route(
            "/api/v1/environment-profiles/{id}",
            get(read).put(replace).delete(remove),
        )
        .with_state(service)
}

async fn create(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Json(body): Json<ProfileRequest>,
) -> Result<(StatusCode, Json<ProfileView>), Problem> {
    let profile = service.create(&caller, body.into()).await?;
    Ok((
        StatusCode::CREATED,
        Json(ProfileView::render(profile, &service)),
    ))
}

async fn list(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<Vec<ProfileView>>, Problem> {
    let profiles = service.list(&caller).await?;
    Ok(Json(
        profiles
            .into_iter()
            .map(|p| ProfileView::render(p, &service))
            .collect(),
    ))
}

async fn read(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<ProfileView>, Problem> {
    let profile = service.get(&caller, &EnvironmentProfileId(id)).await?;
    Ok(Json(ProfileView::render(profile, &service)))
}

async fn replace(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<ProfileRequest>,
) -> Result<Json<ProfileView>, Problem> {
    let profile = service
        .update(&caller, &EnvironmentProfileId(id), body.into())
        .await?;
    Ok(Json(ProfileView::render(profile, &service)))
}

async fn remove(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    service.delete(&caller, &EnvironmentProfileId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn skills(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    // The caller's own credential, when the deployment delegates one. Without
    // it the catalogue answers as this service, which sees only what is public.
    credential: Option<axum::Extension<DelegatedCredential>>,
) -> Result<Json<SkillCatalogView>, Problem> {
    let credential = credential.map(|axum::Extension(c)| c.0);
    let skills = service
        .available_skills(&caller, credential.as_deref())
        .await?;
    Ok(Json(SkillCatalogView {
        skills: skills.into_iter().map(SkillView::from).collect(),
        mandatory: MandatoryView::of(service.baseline()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use layer2_core::{Baseline, CoreError, EnvironmentProfileStore, StoreError, SubjectId};

    /// A store that holds nothing, for the cases that never reach it.
    struct EmptyStore;

    #[async_trait::async_trait]
    impl EnvironmentProfileStore for EmptyStore {
        async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
            Ok(())
        }
        async fn get(
            &self,
            _id: &EnvironmentProfileId,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
        }
        async fn list_for(
            &self,
            _owner: &SubjectId,
        ) -> Result<Vec<EnvironmentProfile>, StoreError> {
            Ok(Vec::new())
        }
        async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
            Ok(())
        }
    }

    fn service() -> Arc<EnvironmentProfileService> {
        EnvironmentProfileService::new(
            Arc::new(EmptyStore),
            Baseline::new(
                [SoftwarePackage::new("platform-cli")],
                [SkillId::new("mandatory-one")],
            ),
            None,
        )
    }

    fn profile() -> EnvironmentProfile {
        EnvironmentProfile {
            id: EnvironmentProfileId("envp_1".into()),
            owner: SubjectId("subject-a".into()),
            name: "build tools".into(),
            variables: vec![EnvironmentVariable::new("BUILD_MODE", "release")],
            packages: vec![SoftwarePackage {
                name: "ripgrep".into(),
                version: Some("14".into()),
            }],
            skills: vec![SkillId::new("mandatory-one"), SkillId::new("chosen")],
            created_at: "2026-08-12T00:00:00Z".into(),
            updated_at: "2026-08-13T00:00:00Z".into(),
        }
    }

    #[test]
    fn the_routes_build_without_conflicting() {
        // The catalogue is a static segment beside an identifier pattern.
        // Building the router is what proves the two can coexist, and a
        // conflict would otherwise surface as a panic at startup.
        let _ = router(service());
    }

    #[test]
    fn a_view_tells_a_client_which_entries_it_may_not_remove() {
        let view = ProfileView::render(profile(), &service());
        let json = serde_json::to_value(&view).expect("serializes");
        assert_eq!(json["mandatory"]["packages"][0], "platform-cli");
        assert_eq!(json["mandatory"]["skills"][0], "mandatory-one");
        assert_eq!(json["id"], "envp_1");
        assert_eq!(json["packages"][0]["version"], "14");
    }

    #[test]
    fn an_unpinned_package_omits_its_version_rather_than_sending_null() {
        let mut p = profile();
        p.packages = vec![SoftwarePackage::new("ripgrep")];
        let json = serde_json::to_value(ProfileView::render(p, &service())).expect("serializes");
        assert!(json["packages"][0].get("version").is_none(), "got {json}");
    }

    #[test]
    fn the_catalogue_states_what_is_mandatory_before_any_profile_exists() {
        // A client authoring its first profile has no profile to read the
        // mandated entries from, and a form that acquires entries the moment
        // it is saved is the confusion this exists to prevent.
        let view = SkillCatalogView {
            skills: Vec::new(),
            mandatory: MandatoryView::of(service().baseline()),
        };
        let json = serde_json::to_value(&view).expect("serializes");
        assert_eq!(json["mandatory"]["packages"][0], "platform-cli");
        assert_eq!(json["mandatory"]["skills"][0], "mandatory-one");
        assert!(json["skills"].as_array().expect("an array").is_empty());
    }

    #[test]
    fn a_request_body_becomes_a_core_spec() {
        let body: ProfileRequest = serde_json::from_str(
            r#"{"name":"mine","variables":[{"key":"A","value":"1"}],"skills":["s1"]}"#,
        )
        .expect("parses");
        let spec: EnvironmentProfileSpec = body.into();
        assert_eq!(spec.name, "mine");
        assert_eq!(spec.variables.len(), 1);
        assert_eq!(spec.variables[0].key, "A");
        assert_eq!(spec.skills, vec![SkillId::new("s1")]);
        // Omitted lists are empty, not an error: a profile that sets only
        // variables is a legitimate thing to ask for.
        assert!(spec.packages.is_empty());
    }

    #[test]
    fn a_profile_a_caller_may_not_see_reads_as_not_found() {
        let response = Problem(CoreError::EnvironmentProfileNotFound).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn an_unusable_request_is_unprocessable_not_a_server_error() {
        let response =
            Problem(CoreError::InvalidRequest("name must not be empty".into())).into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
