//! REST projection of deployment-wide settings.
//!
//! PATCH is conditional and setting-granular: omission leaves one setting
//! alone, while a supplied setting replaces that setting as a whole. `null`
//! has meaning only for the optional live-session limit. The briefing remains
//! an assembled type whose generated granted-services section is not request
//! data and therefore cannot be removed by a request.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::{
    extract::{FromRequest, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    CapacityBounds, ChatAgentSettings, ChatHostSettings, ChatInactivity, ChatSettings, ChatSetup,
    ChatWorkspaceSource, CoreError, DeploymentCapacity, PlatformSettings, PlatformSettingsService,
    SessionBriefing, SessionSize, StoredSettings,
};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::AdminProblem;
use crate::auth::{AuthenticatedCaller, SettingsTouched};
use crate::rest::openapi_shapes;
use crate::rest::{problem_document, problem_response, ConfidentialBodyProblem};

/// The two exact paths owned by this module remain in the parent surface's
/// route list, so its invariant test still covers everything a gateway must
/// restrict.
pub(super) fn router(service: Arc<PlatformSettingsService>) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(read, replace, change))
        .routes(routes!(reset))
        .with_state(service)
}

/// The settings document together with the revision a future write must name.
#[derive(Debug, Serialize, ToSchema)]
struct SettingsView {
    revision: u64,
    #[schema(value_type = openapi_shapes::PlatformSettings)]
    settings: PlatformSettings,
    /// The full object replaced by the most recent write, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<openapi_shapes::PlatformSettings>)]
    previous: Option<PlatformSettings>,
}

impl From<StoredSettings> for SettingsView {
    fn from(stored: StoredSettings) -> Self {
        Self {
            revision: stored.revision,
            settings: stored.settings,
            previous: stored.previous,
        }
    }
}

/// A field on a settings PATCH.
///
/// Serde normally maps both a missing optional field and an explicit `null` to
/// `None`. Those are different acts here: missing means unchanged, while null
/// removes the live-session limit and is invalid for settings that are not
/// optional.
#[derive(Debug, Default)]
enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T> Deserialize<'de> for PatchField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

/// All editable prose around the generated granted-services section.
///
/// Every field is required when the briefing setting is replaced. This keeps
/// a PATCH from silently filling an accidentally omitted field with shipped
/// prose, which would be neither "leave alone" nor what the client sent.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct BriefingRequest {
    before: Vec<String>,
    reach_heading: String,
    reach_lead: String,
    reach_close: String,
    reach_none: String,
    after: Vec<String>,
}

impl From<BriefingRequest> for SessionBriefing {
    fn from(value: BriefingRequest) -> Self {
        Self {
            before: value.before,
            reach_heading: value.reach_heading,
            reach_lead: value.reach_lead,
            reach_close: value.reach_close,
            reach_none: value.reach_none,
            after: value.after,
        }
    }
}

/// The complete range setting. Partial ranges are not meaningful: every
/// minimum must arrive with the maximum it is compared against.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct BoundsRequest {
    #[schema(value_type = u32, minimum = 1)]
    min_cpu_millis: NonZeroU32,
    #[schema(value_type = u32, minimum = 1)]
    max_cpu_millis: NonZeroU32,
    #[schema(value_type = u32, minimum = 1)]
    min_memory_mib: NonZeroU32,
    #[schema(value_type = u32, minimum = 1)]
    max_memory_mib: NonZeroU32,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
struct ChatWorkspaceRequest {
    #[schema(max_length = 512)]
    repository_url: String,
    #[schema(min_length = 1, max_length = 200)]
    branch: String,
}

impl Default for ChatWorkspaceRequest {
    fn default() -> Self {
        let value = ChatWorkspaceSource::default();
        Self {
            repository_url: value.repository_url,
            branch: value.branch,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
struct ChatAgentRequest {
    model: Option<String>,
    #[schema(value_type = String, example = "high")]
    reasoning_effort: layer2_core::ReasoningEffort,
    instructions: String,
    record_tool_bodies: bool,
}

impl Default for ChatAgentRequest {
    fn default() -> Self {
        let value = ChatAgentSettings::default();
        Self {
            model: value.model,
            reasoning_effort: value.reasoning_effort,
            instructions: value.instructions,
            record_tool_bodies: value.record_tool_bodies,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ChatSizeRequest {
    #[schema(minimum = 1)]
    cpu_millis: u32,
    #[schema(minimum = 1)]
    memory_mib: u32,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
struct ChatInactivityRequest {
    #[schema(minimum = 60, maximum = 86400)]
    pause_after_seconds: u32,
}

impl Default for ChatInactivityRequest {
    fn default() -> Self {
        Self {
            pause_after_seconds: ChatInactivity::default().pause_after_seconds,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
struct ChatSetupRequest {
    #[schema(max_items = 16)]
    commands: Vec<String>,
}

impl Default for ChatSetupRequest {
    fn default() -> Self {
        Self {
            commands: ChatSetup::default().commands,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
struct ChatHostRequest {
    size: Option<ChatSizeRequest>,
    #[schema(minimum = 1, maximum = 50)]
    max_threads: u8,
}

impl Default for ChatHostRequest {
    fn default() -> Self {
        Self {
            size: None,
            max_threads: ChatHostSettings::default().max_threads,
        }
    }
}

/// A complete replacement for the optional chat setting. Omitted nested
/// fields use the shipped defaults; unknown fields are refused.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
struct ChatSettingsRequest {
    enabled: bool,
    workspace: ChatWorkspaceRequest,
    agent: ChatAgentRequest,
    #[schema(max_items = 8)]
    message_prefixes: Vec<String>,
    inactivity: ChatInactivityRequest,
    setup: ChatSetupRequest,
    host: ChatHostRequest,
}

impl Default for ChatSettingsRequest {
    fn default() -> Self {
        let value = ChatSettings::default();
        Self {
            enabled: value.enabled,
            workspace: ChatWorkspaceRequest::default(),
            agent: ChatAgentRequest::default(),
            message_prefixes: value.message_prefixes,
            inactivity: ChatInactivityRequest::default(),
            setup: ChatSetupRequest::default(),
            host: ChatHostRequest::default(),
        }
    }
}

#[derive(Debug)]
struct SettingsValidation {
    field: String,
    path: String,
    detail: &'static str,
}

impl SettingsValidation {
    fn new(field: impl Into<String>, path: impl Into<String>, detail: &'static str) -> Self {
        Self {
            field: field.into(),
            path: path.into(),
            detail,
        }
    }
}

impl TryFrom<ChatSettingsRequest> for ChatSettings {
    type Error = SettingsValidation;

    fn try_from(value: ChatSettingsRequest) -> Result<Self, Self::Error> {
        let size = value
            .host
            .size
            .map(|size| {
                SessionSize::new(size.cpu_millis, size.memory_mib).map_err(|_| {
                    SettingsValidation::new(
                        "size",
                        "chat.host.size",
                        "CPU and memory must both be positive",
                    )
                })
            })
            .transpose()?;
        let settings = ChatSettings {
            enabled: value.enabled,
            workspace: ChatWorkspaceSource {
                repository_url: value.workspace.repository_url,
                branch: value.workspace.branch,
            },
            agent: ChatAgentSettings {
                model: value.agent.model,
                reasoning_effort: value.agent.reasoning_effort,
                instructions: value.agent.instructions,
                record_tool_bodies: value.agent.record_tool_bodies,
            },
            message_prefixes: value.message_prefixes,
            inactivity: ChatInactivity {
                pause_after_seconds: value.inactivity.pause_after_seconds,
            },
            setup: ChatSetup {
                commands: value.setup.commands,
            },
            host: ChatHostSettings {
                size,
                max_threads: value.host.max_threads,
            },
        };
        settings.validate().map_err(|violation| {
            SettingsValidation::new(violation.field(), violation.path(), violation.detail())
        })?;
        Ok(settings)
    }
}

impl From<BoundsRequest> for CapacityBounds {
    fn from(value: BoundsRequest) -> Self {
        Self {
            min_cpu_millis: value.min_cpu_millis,
            max_cpu_millis: value.max_cpu_millis,
            min_memory_mib: value.min_memory_mib,
            max_memory_mib: value.max_memory_mib,
        }
    }
}

/// One conditional settings change.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ChangeRequest {
    expected_revision: u64,
    #[serde(default)]
    #[schema(required = false, value_type = BriefingRequest)]
    briefing: PatchField<BriefingRequest>,
    #[serde(default)]
    #[schema(
        required = false,
        value_type = openapi_shapes::DeploymentCapacity
    )]
    deployment_capacity: PatchField<DeploymentCapacity>,
    #[serde(default)]
    #[schema(required = false, value_type = BoundsRequest)]
    size_bounds: PatchField<BoundsRequest>,
    #[serde(default)]
    #[schema(required = false, value_type = Option<u32>, minimum = 1)]
    live_sessions: PatchField<NonZeroU32>,
    #[serde(default)]
    #[schema(required = false, value_type = Option<ChatSettingsRequest>)]
    chat: PatchField<ChatSettingsRequest>,
}

/// A conditional replacement of the complete settings document.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ReplaceRequest {
    expected_revision: u64,
    #[schema(value_type = openapi_shapes::PlatformSettings)]
    settings: serde_json::Value,
}

impl ChangeRequest {
    fn apply(
        self,
        mut settings: PlatformSettings,
    ) -> Result<(u64, PlatformSettings), SettingsFailure> {
        if matches!(&self.briefing, PatchField::Missing)
            && matches!(&self.deployment_capacity, PatchField::Missing)
            && matches!(&self.size_bounds, PatchField::Missing)
            && matches!(&self.live_sessions, PatchField::Missing)
            && matches!(&self.chat, PatchField::Missing)
        {
            return Err(CoreError::InvalidRequest(
                "a settings change must name at least one of 'briefing', \
                 'deployment_capacity', 'size_bounds', 'live_sessions', or 'chat'"
                    .into(),
            )
            .into());
        }

        match self.briefing {
            PatchField::Missing => {}
            PatchField::Null => {
                return Err(CoreError::InvalidRequest(
                    "'briefing' cannot be null; use the reset operation to restore shipped prose"
                        .into(),
                )
                .into());
            }
            PatchField::Value(briefing) => settings.briefing = briefing.into(),
        }
        match self.deployment_capacity {
            PatchField::Missing => {}
            PatchField::Null => {
                return Err(CoreError::InvalidRequest(
                    "'deployment_capacity' cannot be null; send its three nullable fields".into(),
                )
                .into());
            }
            PatchField::Value(capacity) => settings.deployment_capacity = capacity,
        }
        match self.size_bounds {
            PatchField::Missing => {}
            PatchField::Null => {
                return Err(CoreError::InvalidRequest(
                    "'size_bounds' cannot be null; send all four non-zero bounds".into(),
                )
                .into());
            }
            PatchField::Value(bounds) => settings.size_bounds = bounds.into(),
        }
        match self.live_sessions {
            PatchField::Missing => {}
            PatchField::Null => settings.live_sessions = None,
            PatchField::Value(limit) => settings.live_sessions = Some(limit),
        }
        match self.chat {
            PatchField::Missing => {}
            PatchField::Null => settings.chat = None,
            PatchField::Value(chat) => {
                settings.chat = Some(chat.try_into().map_err(SettingsFailure::Validation)?)
            }
        }
        Ok((self.expected_revision, settings))
    }
}

impl ReplaceRequest {
    fn into_settings(mut self) -> Result<(u64, PlatformSettings), SettingsFailure> {
        let object = self
            .settings
            .as_object_mut()
            .ok_or(SettingsFailure::InvalidSchema)?;
        let chat = object.remove("chat");
        let mut settings: PlatformSettings =
            serde_json::from_value(self.settings).map_err(|_| SettingsFailure::InvalidSchema)?;
        settings.chat = match chat {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<ChatSettingsRequest>(value)
                    .map_err(|_| SettingsFailure::InvalidSchema)?
                    .try_into()
                    .map_err(SettingsFailure::Validation)?,
            ),
        };
        Ok((self.expected_revision, settings))
    }
}

/// A conditional request to restore the resettable settings.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
struct ResetRequest {
    expected_revision: u64,
}

/// A settings request can fail while being decoded or after reaching the core.
enum SettingsFailure {
    Body(ConfidentialBodyProblem),
    InvalidSchema,
    Validation(SettingsValidation),
    Refused(AdminProblem),
}

/// A confidential settings body decoded in two passes so unknown chat fields
/// can be named without ever reflecting the value stored beneath them.
struct SettingsJson<T>(T);

impl<S, T> FromRequest<S> for SettingsJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = SettingsFailure;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<serde_json::Value>::from_request(request, state)
            .await
            .map_err(SettingsFailure::from)?;
        if let Some(validation) = unknown_chat_field(&value) {
            return Err(SettingsFailure::Validation(validation));
        }
        serde_json::from_value(value)
            .map(Self)
            .map_err(|_| SettingsFailure::InvalidSchema)
    }
}

fn unknown_chat_field(value: &serde_json::Value) -> Option<SettingsValidation> {
    let (chat, path) = if let Some(chat) = value.get("chat") {
        (chat, "chat")
    } else {
        (value.get("settings")?.get("chat")?, "settings.chat")
    };
    let chat = chat.as_object()?;
    unknown_key(
        chat,
        &[
            "enabled",
            "workspace",
            "agent",
            "message_prefixes",
            "inactivity",
            "setup",
            "host",
        ],
        path,
    )
    .or_else(|| nested_unknown(chat, "workspace", &["repository_url", "branch"], path))
    .or_else(|| {
        nested_unknown(
            chat,
            "agent",
            &[
                "model",
                "reasoning_effort",
                "instructions",
                "record_tool_bodies",
            ],
            path,
        )
    })
    .or_else(|| nested_unknown(chat, "inactivity", &["pause_after_seconds"], path))
    .or_else(|| nested_unknown(chat, "setup", &["commands"], path))
    .or_else(|| nested_unknown(chat, "host", &["size", "max_threads"], path))
    .or_else(|| {
        chat.get("host")
            .and_then(serde_json::Value::as_object)
            .and_then(|host| host.get("size"))
            .and_then(serde_json::Value::as_object)
            .and_then(|size| {
                unknown_key(
                    size,
                    &["cpu_millis", "memory_mib"],
                    &format!("{path}.host.size"),
                )
            })
    })
}

fn nested_unknown(
    parent: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    allowed: &[&str],
    path: &str,
) -> Option<SettingsValidation> {
    parent
        .get(key)
        .and_then(serde_json::Value::as_object)
        .and_then(|object| unknown_key(object, allowed, &format!("{path}.{key}")))
}

fn unknown_key(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    path: &str,
) -> Option<SettingsValidation> {
    object
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
        .map(|key| {
            SettingsValidation::new(
                key,
                format!("{path}.{key}"),
                "is not a recognized chat settings field",
            )
        })
}

impl From<axum::extract::rejection::JsonRejection> for SettingsFailure {
    fn from(error: axum::extract::rejection::JsonRejection) -> Self {
        Self::Body(ConfidentialBodyProblem(error))
    }
}

impl From<CoreError> for SettingsFailure {
    fn from(error: CoreError) -> Self {
        Self::Refused(AdminProblem::from(error))
    }
}

impl IntoResponse for SettingsFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Body(problem) => problem.into_response(),
            Self::InvalidSchema => validation_response(None),
            Self::Validation(validation) => validation_response(Some(validation)),
            Self::Refused(problem) => problem.into_response(),
        }
    }
}

fn validation_response(validation: Option<SettingsValidation>) -> Response {
    let status = StatusCode::UNPROCESSABLE_ENTITY;
    let detail = validation.as_ref().map_or(
        "The JSON body does not match this operation's request schema.",
        |v| v.detail,
    );
    let mut body = problem_document(status, "invalid-request", detail, "invalid_request");
    if let (Some(validation), Some(object)) = (validation, body.as_object_mut()) {
        object.insert("field".into(), validation.field.into());
        object.insert("path".into(), validation.path.into());
    }
    problem_response(status, body)
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/settings",
    tag = "administration",
    responses((status = 200, description = "Deployment settings", body = SettingsView))
)]
async fn read(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<SettingsView>, AdminProblem> {
    Ok(Json(service.read(&caller).await?.into()))
}

#[utoipa::path(
    patch,
    path = "/api/v1/admin/settings",
    tag = "administration",
    request_body = ChangeRequest,
    responses((status = 200, description = "Changed deployment settings", body = SettingsView))
)]
async fn change(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    SettingsJson(body): SettingsJson<ChangeRequest>,
) -> Result<Response, SettingsFailure> {
    let current = service.read(&caller).await?;
    let previous = current.settings.clone();
    let (expected, replacement) = body.apply(current.settings)?;
    let stored = service.write(&caller, expected, replacement).await?;
    Ok(changed_response(stored, &previous))
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/settings",
    tag = "administration",
    request_body = ReplaceRequest,
    responses((status = 200, description = "Replaced deployment settings", body = SettingsView))
)]
async fn replace(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    SettingsJson(body): SettingsJson<ReplaceRequest>,
) -> Result<Response, SettingsFailure> {
    let previous = service.read(&caller).await?.settings;
    let (expected_revision, settings) = body.into_settings()?;
    let stored = service.write(&caller, expected_revision, settings).await?;
    Ok(changed_response(stored, &previous))
}

#[utoipa::path(
    post,
    path = "/api/v1/admin/settings/reset",
    tag = "administration",
    request_body = ResetRequest,
    responses((status = 200, description = "Settings with shipped briefing restored and Chat removed", body = SettingsView))
)]
async fn reset(
    State(service): State<Arc<PlatformSettingsService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    SettingsJson(body): SettingsJson<ResetRequest>,
) -> Result<Response, SettingsFailure> {
    let previous = service.read(&caller).await?.settings;
    let stored = service
        .reset_settings(&caller, body.expected_revision)
        .await?;
    Ok(changed_response(stored, &previous))
}

/// Render a successful write and carry the facts only its handler can know.
fn changed_response(stored: StoredSettings, previous: &PlatformSettings) -> Response {
    let changed = stored.settings.changed_from(previous);
    let rate_card = changed
        .contains(&"pricing")
        .then(|| {
            stored
                .settings
                .pricing
                .in_force()
                .map(|card| card.id.0.clone())
        })
        .flatten();
    let mut response = Json(SettingsView::from(stored)).into_response();
    response
        .extensions_mut()
        .insert(SettingsTouched { changed, rate_card });
    response
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
