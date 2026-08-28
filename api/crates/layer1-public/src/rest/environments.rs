//! REST projection of environment profiles.
//!
//! A plain resource collection: create, read, replace, remove, plus the
//! catalogue a client picks skills from. Ownership, validation and the
//! non-revocable baseline all live in Layer 2, so no interface can apply a
//! different version of them. The preference action also rebuilds its released
//! `/v1` collection response from bounded Layer 2 list pages after the core
//! mutation succeeds.
//!
//! Variable values travel in and out because a client that could not read back
//! what it stored could not offer an edit form. They are never logged here,
//! and the core's own `Debug` keeps them out of a log elsewhere.

use std::{collections::HashSet, sync::Arc};

use axum::{
    body::Bytes,
    extract::{FromRequest, Path, Query, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use layer2_core::{
    Caller, CoreError, EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService,
    PageCursor, PageRequest, MAX_PAGE_SIZE,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::rest::environments_shapes::{
    decode_profile_request, unknown_profile_field, MandatoryView, ProfileRequest, ProfileView,
    SkillCatalogView, SkillView, UnknownProfileField,
};
use crate::rest::pagination::PageParams;
use crate::rest::{problem_document, problem_response, BodyProblem, Problem, RequestFailure};

/// Routes this resource contributes.
///
/// The catalogue sits under the collection because it exists only to be picked
/// from when authoring one; it is a static segment, which a router matches
/// ahead of the identifier pattern beside it.
pub fn router(service: Arc<EnvironmentProfileService>) -> Router {
    documented_router(service).into()
}

pub(super) fn documented_router(service: Arc<EnvironmentProfileService>) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(skills))
        .routes(routes!(services))
        .routes(routes!(read, replace, remove))
        /*
         * Which profile clients should preselect for this owner.
         *
         * A collection rather than a flag on one profile, because setting it
         * changes two records — the one taking it and the one giving it up —
         * and a `PUT` on a single profile would describe only half of that.
         * The released /v1 answer is the complete post-write collection, so a
         * client never has to guess which other rows moved. It is assembled
         * through bounded list pages after the mutation succeeds.
         */
        .routes(routes!(prefer))
        .with_state(service)
}

#[cfg(test)]
#[path = "environments_tests.rs"]
mod route_tests;

/// A profile body gets one additional diagnostic over the shared JSON
/// extractor: an identifier-shaped unknown field is named without reflecting
/// any value that appeared beneath it.
struct ProfileBody {
    request: ProfileRequest,
    echoed_id: Option<String>,
}

enum ProfileBodyProblem {
    Body(BodyProblem),
    InvalidSchema,
    Unknown(UnknownProfileField),
}

impl<S> FromRequest<S> for ProfileBody
where
    S: Send + Sync,
{
    type Rejection = ProfileBodyProblem;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let accepts_profile_view = request.method() == Method::PUT;
        if !json_content_type(request.headers()) {
            return Err(ProfileBodyProblem::Body(
                BodyProblem::missing_json_content_type(),
            ));
        }
        let bytes = Bytes::from_request(request, state).await.map_err(|error| {
            let rejection = axum::extract::rejection::JsonRejection::from(error);
            ProfileBodyProblem::Body(BodyProblem::from(rejection))
        })?;
        let Json(value) = Json::<Value>::from_bytes(&bytes)
            .map_err(|error| ProfileBodyProblem::Body(error.into()))?;
        if let Some(unknown) = unknown_profile_field(&value, accepts_profile_view) {
            return Err(ProfileBodyProblem::Unknown(unknown));
        }
        let (request, echoed_id) = decode_profile_request(value, accepts_profile_view)
            .map_err(|_| ProfileBodyProblem::InvalidSchema)?;
        Ok(Self { request, echoed_id })
    }
}

fn json_content_type(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<mime::Mime>().ok())
    else {
        return false;
    };
    content_type.type_() == mime::APPLICATION
        && (content_type.subtype() == mime::JSON || content_type.suffix() == Some(mime::JSON))
}

impl IntoResponse for ProfileBodyProblem {
    fn into_response(self) -> Response {
        match self {
            Self::Body(problem) => problem.into_response(),
            Self::InvalidSchema => BodyProblem::invalid_data().into_response(),
            Self::Unknown(unknown) => {
                let status = StatusCode::UNPROCESSABLE_ENTITY;
                let mut body = problem_document(
                    status,
                    "invalid-request",
                    "The JSON body does not match this operation's request schema.",
                    "invalid_request",
                );
                if let Some(object) = body.as_object_mut() {
                    if let Some(field) = unknown.field {
                        object.insert("field".into(), field.into());
                    }
                    if let Some(path) = unknown.path {
                        object.insert("path".into(), path.into());
                    }
                }
                problem_response(status, body)
            }
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/environment-profiles",
    tag = "environment profiles",
    request_body = ProfileRequest,
    responses((status = 201, description = "Profile created", body = ProfileView))
)]
async fn create(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    ProfileBody { request, .. }: ProfileBody,
) -> Result<(StatusCode, Json<ProfileView>), RequestFailure> {
    let profile = service.create(&caller, request.try_into()?).await?;
    Ok((
        StatusCode::CREATED,
        Json(ProfileView::render(profile, &service)),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/environment-profiles",
    tag = "environment profiles",
    params(("limit" = Option<u32>, Query), ("cursor" = Option<String>, Query)),
    responses((status = 200, description = "Newest-first profile page", body = [ProfileView]))
)]
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

/// Which profile clients should preselect for the caller.
///
/// `null` is a value rather than an omission: it clears the preselection. The
/// server never substitutes this value into a session-create request;
/// `deny_unknown_fields` ensures a misspelling is reported instead of silently
/// clearing it.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct PreferredRequest {
    profile: Option<String>,
}

#[utoipa::path(
    put,
    path = "/api/v1/environment-profiles/preferred",
    tag = "environment profiles",
    request_body = PreferredRequest,
    responses((status = 200, description = "Complete profile collection after preference change", body = [ProfileView]))
)]
async fn prefer(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    body: Result<Json<PreferredRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Vec<ProfileView>>, RequestFailure> {
    let Json(body) = body?;
    let wanted = body.profile.map(EnvironmentProfileId);
    service.prefer(&caller, wanted.as_ref()).await?;
    let profiles = complete_profile_collection(&service, &caller, wanted.as_ref()).await?;
    Ok(Json(
        profiles
            .into_iter()
            .map(|p| ProfileView::render(p, &service))
            .collect(),
    ))
}

/// Rebuild the released `/v1` action response without an unbounded store read.
async fn complete_profile_collection(
    service: &EnvironmentProfileService,
    caller: &Caller,
    pinned_preferred: Option<&EnvironmentProfileId>,
) -> Result<Vec<EnvironmentProfile>, CoreError> {
    let mut profiles = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    let mut seen = HashSet::new();
    loop {
        let mut page = service
            .list(caller, &PageRequest::new(MAX_PAGE_SIZE, cursor))
            .await?;
        // Another preference write may land between pages. `/v1` promises the
        // post-write answer for this request, so every page is normalized to
        // the winner this request validated and wrote rather than whichever
        // winner happened to be visible during that individual page read.
        for profile in &mut page.items {
            profile.preferred = pinned_preferred == Some(&profile.id);
        }
        profiles.extend(page.items);
        match page.next {
            Some(next) if seen.insert(next.clone()) => cursor = Some(next),
            Some(_) => {
                return Err(CoreError::RuntimeUnavailable(
                    "environment profile listing repeated its continuation cursor".into(),
                ))
            }
            None => return Ok(profiles),
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/environment-profiles/{id}",
    tag = "environment profiles",
    params(("id" = String, Path)),
    responses((status = 200, description = "Profile", body = ProfileView))
)]
async fn read(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<ProfileView>, Problem> {
    let profile = service.get(&caller, &EnvironmentProfileId(id)).await?;
    Ok(Json(ProfileView::render(profile, &service)))
}

#[utoipa::path(
    put,
    path = "/api/v1/environment-profiles/{id}",
    tag = "environment profiles",
    params(("id" = String, Path)),
    request_body = ProfileRequest,
    responses((status = 200, description = "Replaced profile", body = ProfileView))
)]
async fn replace(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    ProfileBody { request, echoed_id }: ProfileBody,
) -> Result<Json<ProfileView>, RequestFailure> {
    if echoed_id.as_deref().is_some_and(|echoed| echoed != id) {
        return Err(BodyProblem::invalid_data().into());
    }
    let profile = service
        .update(&caller, &EnvironmentProfileId(id), request.try_into()?)
        .await?;
    Ok(Json(ProfileView::render(profile, &service)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/environment-profiles/{id}",
    tag = "environment profiles",
    params(("id" = String, Path)),
    responses((status = 204, description = "Profile deleted"))
)]
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
#[utoipa::path(
    get,
    path = "/api/v1/environment-profiles/services",
    tag = "environment profiles",
    responses((status = 200, description = "Available service catalogue", body = ServiceCatalogView))
)]
async fn services(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::Extension<DelegatedCredential>>,
) -> Result<Json<ServiceCatalogView>, Problem> {
    let credential = credential.map(|axum::Extension(c)| c.into_value());
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
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceView {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// The picker's answer.
#[derive(Debug, Serialize, ToSchema)]
pub struct ServiceCatalogView {
    services: Vec<ServiceView>,
    /// Which of them every session reaches whatever the profile says.
    mandatory: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/environment-profiles/skills",
    tag = "environment profiles",
    responses((status = 200, description = "Available skill catalogue", body = SkillCatalogView))
)]
async fn skills(
    State(service): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    // The caller's own credential, when the deployment delegates one. Without
    // it the catalogue answers as this service, which sees only what is public.
    credential: Option<axum::Extension<DelegatedCredential>>,
) -> Result<Json<SkillCatalogView>, Problem> {
    let credential = credential.map(|axum::Extension(c)| c.into_value());
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        async fn replace_content(
            &self,
            _id: &EnvironmentProfileId,
            _content: &EnvironmentProfileSpec,
            _updated_at: &str,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
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

    #[tokio::test]
    async fn the_compatibility_walk_rejects_a_longer_cursor_cycle() {
        #[derive(Default)]
        struct CyclingStore {
            lists: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl EnvironmentProfileStore for CyclingStore {
            async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
                Ok(())
            }

            async fn replace_content(
                &self,
                _id: &EnvironmentProfileId,
                _content: &EnvironmentProfileSpec,
                _updated_at: &str,
            ) -> Result<Option<EnvironmentProfile>, StoreError> {
                Ok(None)
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
                self.lists.fetch_add(1, Ordering::SeqCst);
                let id = match page.cursor().map(|cursor| cursor.id.as_str()) {
                    None => "cursor-a",
                    Some("cursor-a") => "cursor-b",
                    Some("cursor-b") => "cursor-a",
                    Some(_) => {
                        return Err(StoreError::Malformed(
                            "the test received an unexpected cursor".into(),
                        ))
                    }
                };
                Ok(Page {
                    items: Vec::new(),
                    next: Some(PageCursor::new("2026-08-25T00:00:00Z", id)),
                })
            }

            async fn set_preferred(
                &self,
                _owner: &SubjectId,
                chosen: Option<&EnvironmentProfileId>,
            ) -> Result<(), StoreError> {
                if chosen.is_some() {
                    return Err(StoreError::Malformed(
                        "the cycling store has no selectable profile".into(),
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

        let store = Arc::new(CyclingStore::default());
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

        let failure = match prefer(
            State(service),
            AuthenticatedCaller(caller),
            Ok(Json(PreferredRequest { profile: None })),
        )
        .await
        {
            Err(failure) => failure,
            Ok(_) => panic!("a repeated cursor must not produce a partial collection"),
        };

        match failure {
            RequestFailure::Refused(Problem(CoreError::RuntimeUnavailable(detail))) => {
                assert!(detail.contains("repeated its continuation cursor"));
            }
            _ => panic!("a cursor cycle must be reported as store unavailability"),
        }
        assert_eq!(store.lists.load(Ordering::SeqCst), 3);
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

            async fn replace_content(
                &self,
                id: &EnvironmentProfileId,
                content: &EnvironmentProfileSpec,
                updated_at: &str,
            ) -> Result<Option<EnvironmentProfile>, StoreError> {
                let mut profiles = self.0.lock().unwrap();
                let Some(profile) = profiles.iter_mut().find(|profile| profile.id == *id) else {
                    return Ok(None);
                };
                profile.apply_content(content, updated_at);
                Ok(Some(profile.clone()))
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

        let refusal = create(
            State(service),
            AuthenticatedCaller(caller),
            ProfileBody {
                request: body,
                echoed_id: None,
            },
        )
        .await;
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
