use super::AgentV2State;
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::sandbox::task_experience::TaskExperienceCapability;
use serde::Serialize;

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct TaskExperienceCapabilities {
    task_home_admission: bool,
    suggested_followup_admission: bool,
    desktop_enable: bool,
    browser_takeover: bool,
    replay_presentation: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct TaskExperienceView {
    object: &'static str,
    schema_version: u16,
    account_id: String,
    capabilities: TaskExperienceCapabilities,
}

#[utoipa::path(get, path = "/api/v2/ui/task-experience", tag = "agent events v2",
    responses((status = 200, body = TaskExperienceView), (status = 401, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    auth: AuthenticatedResourceCaller,
) -> Response {
    use TaskExperienceCapability::*;
    let allows = |capability| {
        state
            .task_experience
            .allows(&auth.context.caller.subject, capability)
    };
    let mut response = Json(TaskExperienceView {
        object: "cma.ui.task_experience",
        schema_version: 1,
        account_id: auth.context.caller.subject.to_string(),
        capabilities: TaskExperienceCapabilities {
            task_home_admission: allows(TaskHomeAdmission),
            suggested_followup_admission: allows(SuggestedFollowupAdmission),
            desktop_enable: allows(DesktopEnable),
            browser_takeover: allows(BrowserTakeover),
            replay_presentation: allows(ReplayPresentation),
        },
    })
    .into_response();
    response
        .headers_mut()
        .insert("cache-control", "private, no-store".parse().unwrap());
    response
}
