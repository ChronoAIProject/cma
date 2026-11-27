use super::*;
use layer2_core::sandbox::agent_environment::{
    AgentEnvironment, EnvironmentAvailability, EnvironmentFeature,
};

#[derive(Serialize, ToSchema)]
pub(crate) struct FeatureView {
    state: &'static str,
    reason: &'static str,
    scope: &'static str,
    writable: bool,
}

impl From<EnvironmentFeature> for FeatureView {
    fn from(value: EnvironmentFeature) -> Self {
        Self {
            state: match value.state {
                EnvironmentAvailability::Available => "available",
                EnvironmentAvailability::Unsupported => "unsupported",
                EnvironmentAvailability::NotEnabled => "not_enabled",
                EnvironmentAvailability::Forbidden => "forbidden",
                EnvironmentAvailability::TemporarilyUnavailable => "temporarily_unavailable",
            },
            reason: value.reason,
            scope: value.scope,
            writable: value.writable,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub(crate) struct EnvironmentView {
    agent_id: String,
    policy_revision: u64,
    runtime_incarnation: Option<String>,
    /// Earliest requested refresh, including pending capability observations.
    revalidate_at: Option<String>,
    /// Deadline for the access decision, independent of capability retries.
    access_revalidate_at: Option<String>,
    files: FeatureView,
    operation_output: FeatureView,
    browser: FeatureView,
    replay: FeatureView,
    control: FeatureView,
}

impl From<AgentEnvironment> for EnvironmentView {
    fn from(value: AgentEnvironment) -> Self {
        Self {
            agent_id: value.agent_id,
            policy_revision: value.policy_revision,
            runtime_incarnation: value.runtime_incarnation,
            revalidate_at: value.revalidate_at,
            access_revalidate_at: value.access_revalidate_at,
            files: value.files.into(),
            operation_output: value.operation_output.into(),
            browser: value.browser.into(),
            replay: value.replay.into(),
            control: value.control.into(),
        }
    }
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/environment", tag = "agents",
    params(("id" = String, Path)), responses((status = 200, body = EnvironmentView)))]
pub(super) async fn read(
    State(service): State<Arc<AgentFileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    Path(id): Path<String>,
) -> Result<Json<EnvironmentView>, FileProblem> {
    let value = service
        .environment(
            &caller,
            context(&resource),
            credential(&resource),
            &AgentId(id),
        )
        .await?;
    Ok(Json(value.into()))
}
