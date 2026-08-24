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
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use layer2_core::{EnvironmentProfileId, EnvironmentProfileService};
use serde::{Deserialize, Serialize};

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::rest::environments_shapes::{
    MandatoryView, ProfileRequest, ProfileView, SkillCatalogView, SkillView,
};
use crate::rest::pagination::PageParams;
use crate::rest::{Problem, RequestFailure};

/// Routes this resource contributes.
///
/// The catalogue sits under the collection because it exists only to be picked
/// from when authoring one; it is a static segment, which a router matches
/// ahead of the identifier pattern beside it.
pub fn router(service: Arc<EnvironmentProfileService>) -> Router {
    Router::new()
        .route("/api/v1/environment-profiles", get(list).post(create))
        .route("/api/v1/environment-profiles/skills", get(skills))
        .route("/api/v1/environment-profiles/services", get(services))
        .route(
            "/api/v1/environment-profiles/{id}",
            get(read).put(replace).delete(remove),
        )
        /*
         * Which profile this owner's sessions start from.
         *
         * A collection rather than a flag on one profile, because setting it
         * changes two records — the one taking it and the one giving it up —
         * and a `PUT` on a single profile would describe only half of that.
         * The answer is the bounded change set, so a client can update both
         * rows without this mutation becoming another unbounded listing.
         */
        .route("/api/v1/environment-profiles/preferred", put(prefer))
        .with_state(service)
}

async fn create(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<ProfileRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<ProfileView>), RequestFailure> {
    let Json(body) = body?;
    let profile = service.create(&caller, body.try_into()?).await?;
    Ok((
        StatusCode::CREATED,
        Json(ProfileView::render(profile, &service)),
    ))
}

async fn list(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<(axum::http::HeaderMap, Json<Vec<ProfileView>>), RequestFailure> {
    let Query(params) = params?;
    let profiles = service.list(&caller, &params.store_request()).await?;
    let headers = params.next_headers("/api/v1/environment-profiles", profiles.next.as_ref());
    Ok((
        headers,
        Json(
            profiles
                .items
                .into_iter()
                .map(|p| ProfileView::render(p, &service))
                .collect(),
        ),
    ))
}

/// Which profile the caller's sessions should start from.
///
/// `null` is a value rather than an omission: it clears the preference and
/// returns them to the deployment's own defaults, which is a choice somebody
/// makes rather than the absence of one. `deny_unknown_fields` so a caller who
/// spells the field wrong is told, instead of silently clearing it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferredRequest {
    profile: Option<String>,
}

async fn prefer(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<PreferredRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Vec<ProfileView>>, RequestFailure> {
    let Json(body) = body?;
    let wanted = body.profile.map(EnvironmentProfileId);
    let changed = service.prefer(&caller, wanted.as_ref()).await?;
    Ok(Json(
        changed
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
    body: Result<Json<ProfileRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ProfileView>, RequestFailure> {
    let Json(body) = body?;
    let profile = service
        .update(&caller, &EnvironmentProfileId(id), body.try_into()?)
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

/// The services a caller may allow a session to reach.
///
/// Answered as the caller: which services exist is a fact about their account,
/// and this service's own identity would answer with a different one. The
/// mandatory entries travel alongside so a client can draw them as fixed
/// rather than inferring which of them cannot be unticked.
async fn services(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::Extension<DelegatedCredential>>,
) -> Result<Json<ServiceCatalogView>, Problem> {
    let credential = credential.map(|axum::Extension(c)| c.0);
    let services = service
        .available_services(&caller, credential.as_deref())
        .await?;
    Ok(Json(ServiceCatalogView {
        services: services
            .into_iter()
            .map(|s| ServiceView {
                id: s.id.0,
                name: s.name,
                description: s.description,
            })
            .collect(),
        mandatory: service
            .baseline()
            .services()
            .iter()
            .map(|s| s.0.clone())
            .collect(),
    }))
}

/// A service as the picker renders it.
#[derive(Debug, Serialize)]
pub struct ServiceView {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// The picker's answer.
#[derive(Debug, Serialize)]
pub struct ServiceCatalogView {
    services: Vec<ServiceView>,
    /// Which of them every session reaches whatever the profile says.
    mandatory: Vec<String>,
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
    use layer2_core::{
        CredentialMode, EnvironmentProfile, EnvironmentProfileSpec, EnvironmentVariable,
        PermissionMode, ReasoningEffort, SessionSize, SkillId, SoftwarePackage,
    };

    use super::*;
    use axum::response::IntoResponse;
    use layer2_core::{
        Baseline, CoreError, EnvironmentProfileStore, Page, PageRequest, StoreError, SubjectId,
    };

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
            _page: &PageRequest,
        ) -> Result<Page<EnvironmentProfile>, StoreError> {
            Ok(Page {
                items: Vec::new(),
                next: None,
            })
        }
        async fn set_preferred(
            &self,
            _owner: &SubjectId,
            chosen: Option<&EnvironmentProfileId>,
        ) -> Result<(), StoreError> {
            if chosen.is_some() {
                return Err(StoreError::Malformed(
                    "an empty store has no selectable profile".into(),
                ));
            }
            Ok(())
        }
        async fn preferred_for(
            &self,
            _owner: &SubjectId,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
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
                [],
            ),
            None,
            None,
            Default::default(),
            Arc::new(layer2_core::PlatformSettingsService::unconfigured()),
        )
    }

    fn profile() -> EnvironmentProfile {
        EnvironmentProfile {
            preferred: false,
            id: EnvironmentProfileId("envp_1".into()),
            owner: SubjectId("subject-a".into()),
            name: "build tools".into(),
            variables: vec![EnvironmentVariable::new("BUILD_MODE", "release")],
            packages: vec![SoftwarePackage {
                name: "ripgrep".into(),
                version: Some("14".into()),
            }],
            services: Vec::new(),
            agent_notes: String::new(),
            agent: layer2_core::AgentConfiguration {
                skills: vec![SkillId::new("mandatory-one"), SkillId::new("chosen")],
                ..Default::default()
            },
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
        let mut profile = profile();
        profile.agent.size = Some(SessionSize::new(1_500, 3_072).expect("valid size"));
        let view = ProfileView::render(profile, &service());
        let json = serde_json::to_value(&view).expect("serializes");
        assert_eq!(json["mandatory"]["packages"][0], "platform-cli");
        assert_eq!(json["mandatory"]["skills"][0], "mandatory-one");
        assert_eq!(json["id"], "envp_1");
        assert_eq!(json["packages"][0]["version"], "14");
        assert_eq!(json["agent"]["size"]["cpu_millis"], 1_500);
        assert_eq!(json["agent"]["size"]["memory_mib"], 3_072);
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
            r#"{"name":"mine","variables":[{"key":"A","value":"1"}],
                "agent":{"skills":["s1"],"model":"m","reasoning_effort":"low",
                         "permission":"autonomous","credential":"own"}}"#,
        )
        .expect("parses");
        let spec: EnvironmentProfileSpec = body.try_into().expect("a usable profile");
        assert_eq!(spec.name, "mine");
        assert_eq!(spec.variables.len(), 1);
        assert_eq!(spec.variables[0].key, "A");
        assert_eq!(spec.agent.skills, vec![SkillId::new("s1")]);
        assert_eq!(spec.agent.model.as_deref(), Some("m"));
        assert_eq!(spec.agent.reasoning_effort, ReasoningEffort::Low);
        assert_eq!(spec.agent.permission, PermissionMode::Autonomous);
        assert_eq!(spec.agent.credential, CredentialMode::Own);
        // Omitted lists are empty, not an error: a profile that sets only
        // variables is a legitimate thing to ask for.
        assert!(spec.packages.is_empty());
    }

    #[test]
    fn a_request_body_carries_its_size_into_the_core() {
        let body: ProfileRequest = serde_json::from_str(
            r#"{"name":"mine","agent":{"size":{"cpu_millis":1500,"memory_mib":3072}}}"#,
        )
        .expect("parses");
        let spec: EnvironmentProfileSpec = body.try_into().expect("a usable profile");

        assert_eq!(
            spec.agent.size,
            Some(SessionSize::new(1_500, 3_072).unwrap())
        );
    }

    #[test]
    fn a_request_body_without_a_size_uses_the_deployments_own() {
        let body: ProfileRequest = serde_json::from_str(r#"{"name":"mine"}"#).expect("parses");
        let spec: EnvironmentProfileSpec = body.try_into().expect("a usable profile");

        assert_eq!(spec.agent.size, None);
    }

    #[tokio::test]
    async fn a_zero_size_is_refused_before_the_profile_store_is_written() {
        #[derive(Default)]
        struct RecordingStore(std::sync::Mutex<Vec<EnvironmentProfile>>);

        #[async_trait::async_trait]
        impl EnvironmentProfileStore for RecordingStore {
            async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError> {
                self.0.lock().unwrap().push(profile.clone());
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
                page: &PageRequest,
            ) -> Result<Page<EnvironmentProfile>, StoreError> {
                let mut profiles = self.0.lock().unwrap().clone();
                profiles.truncate(page.limit() as usize);
                Ok(Page {
                    items: profiles,
                    next: None,
                })
            }

            async fn set_preferred(
                &self,
                owner: &SubjectId,
                chosen: Option<&EnvironmentProfileId>,
            ) -> Result<(), StoreError> {
                let mut profiles = self.0.lock().unwrap();
                if let Some(id) = chosen {
                    let selected = profiles
                        .iter()
                        .find(|profile| profile.id == *id && &profile.owner == owner);
                    if selected.is_none() {
                        return Err(StoreError::Malformed(
                            "selected environment profile is not owned by the subject".into(),
                        ));
                    }
                }
                for profile in profiles
                    .iter_mut()
                    .filter(|profile| &profile.owner == owner)
                {
                    profile.preferred = chosen.is_some_and(|id| profile.id == *id);
                }
                Ok(())
            }

            async fn preferred_for(
                &self,
                owner: &SubjectId,
            ) -> Result<Option<EnvironmentProfile>, StoreError> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|profile| &profile.owner == owner && profile.preferred)
                    .max_by(|a, b| {
                        a.updated_at
                            .cmp(&b.updated_at)
                            .then_with(|| a.id.0.cmp(&b.id.0))
                    })
                    .cloned())
            }

            async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
                Ok(())
            }
        }

        let store = Arc::new(RecordingStore::default());
        let service = EnvironmentProfileService::new(
            Arc::clone(&store) as Arc<dyn EnvironmentProfileStore>,
            Baseline::default(),
            None,
            None,
            Default::default(),
            Arc::new(layer2_core::PlatformSettingsService::unconfigured()),
        );
        let caller = layer2_core::Caller::new(
            SubjectId("subject-a".into()),
            layer2_core::Authorization::from_capabilities([
                layer2_core::Capability::ManageOwnEnvironmentProfiles,
            ]),
        );
        let body: ProfileRequest = serde_json::from_str(
            r#"{"name":"mine","agent":{"size":{"cpu_millis":0,"memory_mib":512}}}"#,
        )
        .expect("parses");

        let refusal = create(State(service), AuthenticatedCaller(caller), Ok(Json(body))).await;
        let problem = match refusal {
            Err(problem) => problem,
            Ok(_) => panic!("a zero size must be refused"),
        };
        assert!(matches!(
            &problem,
            RequestFailure::Refused(Problem(CoreError::InvalidSessionSize(_)))
        ));
        let response = problem.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem documents are small");
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).expect("a problem document is json");
        assert_eq!(body["code"], "invalid_session_size");
        assert!(store.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_body_that_says_nothing_about_the_agent_gets_this_deployments_answer() {
        // A client that only wants to name some variables must not thereby
        // choose a model or how freely the agent may act. Absent means
        // "whatever is ordinary here", never "empty".
        let body: ProfileRequest = serde_json::from_str(r#"{"name":"mine"}"#).expect("parses");
        let spec: EnvironmentProfileSpec = body.try_into().expect("a usable profile");
        // The kernel is the exception, and deliberately: this layer does not
        // know which kernels a deployment can start, so it leaves it unset and
        // the core settles it from configuration.
        assert!(
            spec.agent.kernel.is_unset(),
            "naming a kernel here would put a runtime's name in a layer that must not know one"
        );
        assert_eq!(spec.agent.reasoning_effort, ReasoningEffort::High);
        assert_eq!(
            spec.agent.permission,
            PermissionMode::Guarded,
            "a profile that said nothing must still stop to ask"
        );
        assert_eq!(spec.agent.credential, CredentialMode::Delegated);
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
