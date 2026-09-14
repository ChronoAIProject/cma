//! OpenAPI generation for the currently composed REST boundary.

use utoipa::{openapi, OpenApi};

mod projections;

use self::projections::{
    EmbedContentSecurityPolicy, EmbedDescriptor, EmbedSnippets, SignupOptions,
};
use super::admin::profiles::{ProfilePageView, ProfileSummaryView};
use super::admin::sandboxes::{
    AgentSummaryView, SandboxDetailView, SandboxPageView, SandboxSummaryView, WorkspaceSummaryView,
};
use super::admin::settings::{
    InactivitySettingsBody, MarketplaceSettingsBody, PatchRequest, PlatformDefaultsBody,
    PlatformLimitsBody, PlatformSettingsBody, ProfileReferenceBody, ReplaceRequest, ResetRequest,
    RetentionSettingsBody, SettingsView, TriggerSettingsBody,
};
use super::admin::{ActivityPageView, ActivityView, RuntimeEntryView, RuntimeInventoryView};
use super::agent_openapi_shapes::*;
use super::instances_openapi_shapes::{
    CreateSandboxRequest, CreateWorkspaceRequest, FailureView, OwnerAgentSummaryView,
    OwnerSandboxDetailView, OwnerSandboxPageView, OwnerSandboxSummaryView,
    OwnerWorkspaceSummaryView, ProfileReferenceView, SandboxEventView, SandboxLimitsView,
    SandboxView, WorkspaceDetailView, WorkspaceListView, WorkspaceSourceView, WorkspaceView,
    WorktreeListView, WorktreeView,
};
use super::launch_events::*;
use super::openapi_shapes::ConformanceDocument;
use super::problems::ProblemDocument;
use super::profile_authoring::shapes::*;
use super::profiles_openapi_shapes::*;
use super::trigger_openapi_shapes::*;

#[derive(OpenApi)]
#[openapi(
    paths(
        super::conformance,
        super::resource_access::effective,
        super::resource_access::list,
        super::resource_access::read,
        super::resource_access::create,
        super::resource_access::update,
        super::resource_access::revoke,
        super::defaults::read,
        super::profile_authoring::bounds,
        super::profile_authoring::services,
        super::profile_authoring::skills,
        super::profile_authoring::resolve_repository,
        crate::pty::terminal,
        projections::signup_options,
        super::triggers::create,
        super::triggers::list,
        super::triggers::read,
        super::triggers::update,
        super::triggers::delete_trigger,
        super::triggers::publish,
        super::triggers::retire,
        super::triggers::revisions,
        super::triggers::statistics,
        super::triggers::preview,
        super::trigger_entry::open,
        super::trigger_entry::consume,
        super::trigger_placements::create,
        super::trigger_placements::list,
        super::trigger_placements::retire,
        super::launches::create,
        super::launches::list,
        super::launches::read,
        super::launches::events,
        super::launches::requirements,
        super::launches::start,
        super::launches::cancel,
        super::service_connections::create,
        super::service_connections::read,
        projections::badge,
        projections::embed,
        projections::launch_event_companion,
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
        super::agents::create,
        super::agents::list,
        super::agents::read,
        super::agent_source::read,
        super::agents::delete_agent,
        super::agent_operate::input,
        super::agent_operate::steer,
        super::agent_operate::interrupt,
        super::agent_operate::approval,
        super::agent_operate::stop,
        super::agent_operate::events,
        super::agent_operate::transcript,
        super::agent_operate::usage,
        super::agent_operate::resources,
        super::sandboxes::create,
        super::sandboxes::list,
        super::sandboxes::read,
        super::sandboxes::download_snapshot,
        super::sandboxes::remove,
        super::sandboxes::pause,
        super::sandboxes::revive,
        super::sandbox_service_access::grant,
        super::sandboxes::events,
        super::workspaces::create,
        super::workspaces::list,
        super::workspaces::read,
        super::workspaces::remove,
        super::worktrees::list,
        super::worktrees::read,
        super::admin::settings::read,
        super::admin::settings::replace,
        super::admin::settings::patch,
        super::admin::settings::reset,
        super::admin::sandboxes::list,
        super::admin::sandboxes::read,
        super::admin::sandboxes::events,
        super::admin::sandboxes::retire,
        super::admin::profiles::list,
        super::admin::profiles::retire,
        super::admin::triggers::pause,
        super::admin::triggers::resume,
        super::admin::runtime_inventory,
        super::admin::activity,
    ),
    components(schemas(
        ConformanceDocument,
        super::defaults::DefaultProfilesView,
        ProblemDocument,
        ProfileCapacityBoundsView,
        AuthoringServiceView,
        AuthoringSkillView,
        AuthoringSkillPage,
        ResolveRepositoryRequest,
        RepositoryResolutionView,
        SignupOptions,
        EmbedDescriptor,
        EmbedContentSecurityPolicy,
        EmbedSnippets,
        TriggerButtonLabelValue,
        TriggerThemeValue,
        SandboxReusePolicyValue,
        WorkspaceReusePolicyValue,
        AgentReusePolicyValue,
        TriggerPresentationBody,
        TriggerTargetBody,
        TriggerReuseBody,
        TriggerConsentBody,
        TriggerContentBody,
        TriggerQuickMessageBody,
        super::agent_source::AgentSourceView,
        super::agent_source::AgentOriginView,
        super::agent_source::AgentTriggerSourceView,
        CreateTriggerRequest,
        UpdateTriggerRequest,
        PublishTriggerRequest,
        CreatePlacementRequest,
        CreateLaunchRequest,
        StartLaunchRequest,
        CreateServiceConnectionRequest,
        TriggerAvailabilityValue,
        TriggerStatsView,
        PlacementView,
        TriggerRevisionView,
        PublishedTriggerRevisionView,
        TriggerView,
        TriggerPageView,
        TriggerPlacementStatsView,
        TriggerStatisticsView,
        TriggerTrustDisclosureView,
        TriggerTrustPreviewView,
        TriggerSizePreviewView,
        TriggerTechnicalPreviewView,
        TriggerPreviewView,
        ResolvedResourceView,
        LaunchResolutionView,
        ServiceRequirementView,
        ServiceConnectionModeValue,
        RequirementsSnapshotView,
        ReviewNeedView,
        ReviewRequirementsView,
        LaunchTrustResourceValue,
        LaunchTrustDisclosureView,
        LaunchTrustManifestView,
        LaunchConsentSurfaceValue,
        LaunchConfirmationView,
        LaunchView,
        LaunchPageView,
        LaunchEventPageView,
        ServiceConnectionStateValue,
        CreatedServiceConnectionView,
        ServiceConnectionStatusView,
        LaunchStateValue,
        LaunchStepKeyValue,
        LaunchStepStatusValue,
        RepeatChoiceValue,
        ServiceRequirementStateValue,
        FirstMessageStateValue,
        LaunchEffectKindValue,
        LaunchStepView,
        LaunchBlockedReasonView,
        LaunchFailureView,
        LaunchEventKindView,
        LaunchEventView,
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
        SandboxProfileAvailabilityBody,
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
        ResolvedProfileRevisionView,
        SandboxProfileTrustView,
        MarketplaceEntryView,
        ForkedProfileView,
        AgentProfileReferenceBody,
        CreateAgentRequest,
        AgentTextRequest,
        ApprovalDecisionBody,
        ApprovalDecisionRequest,
        OwnerStopReasonBody,
        StopAgentRequest,
        AgentProfileSummaryView,
        AgentSkillSummaryView,
        WorktreeSummaryView,
        WorkspaceContextView,
        SandboxContextView,
        InstanceProfileReferenceView,
        FirstMessageStateView,
        AgentListItemView,
        AgentPageView,
        AgentView,
        AgentFailureView,
        InputAcceptedView,
        AgentEventPageView,
        AgentEventView,
        AgentTranscriptView,
        AgentTranscriptTurnView,
        AgentTranscriptMessageView,
        AgentTranscriptRoleView,
        AgentUsageView,
        AgentResourcesView,
        ProfileReferenceView,
        CreateSandboxRequest,
        super::instances_openapi_shapes::GrantSandboxServiceAccessRequest,
        CreateWorkspaceRequest,
        FailureView,
        SandboxLimitsView,
        super::instances_openapi_shapes::SandboxLifecycleView,
        super::instances_openapi_shapes::SandboxLifecycleEligibilityView,
        super::instances_openapi_shapes::SandboxCheckpointView,
        super::instances_openapi_shapes::SandboxBackgroundSaveView,
        super::instances_openapi_shapes::SandboxCompletedSnapshotView,
        super::instances_openapi_shapes::SandboxLifecycleOperationView,
        SandboxView,
        OwnerSandboxSummaryView,
        OwnerSandboxPageView,
        OwnerWorkspaceSummaryView,
        OwnerSandboxDetailView,
        WorkspaceSourceView,
        WorkspaceView,
        WorktreeView,
        OwnerAgentSummaryView,
        WorkspaceDetailView,
        WorkspaceListView,
        WorktreeListView,
        SandboxEventView,
        ProfileReferenceBody,
        PlatformDefaultsBody,
        InactivitySettingsBody,
        RetentionSettingsBody,
        PlatformLimitsBody,
        TriggerSettingsBody,
        MarketplaceSettingsBody,
        PlatformSettingsBody,
        ReplaceRequest,
        PatchRequest,
        ResetRequest,
        SettingsView,
        SandboxSummaryView,
        SandboxPageView,
        WorkspaceSummaryView,
        AgentSummaryView,
        SandboxDetailView,
        ProfileSummaryView,
        ProfilePageView,
        RuntimeEntryView,
        RuntimeInventoryView,
        ActivityView,
        ActivityPageView,
    )),
    tags(
        (name = "discovery", description = "Anonymous deployment discovery"),
        (name = "triggers", description = "Publisher-owned Trigger drafts and public previews"),
        (name = "trigger placements", description = "Owned Trigger placement and embed projections"),
        (name = "trigger launches", description = "Owned Trigger Launch lifecycle and event replay"),
        (name = "service connections", description = "Owner-scoped service authorization handoffs"),
        (name = "profile authoring", description = "Read-only profile choices and repository verification"),
        (name = "agent profiles", description = "Owned Agent Profile drafts and immutable revisions"),
        (name = "workspace profiles", description = "Owned Workspace Profile drafts and immutable revisions"),
        (name = "sandbox profiles", description = "Owned Sandbox Profile drafts and immutable revisions"),
        (name = "profile marketplace", description = "Published profiles available to authenticated callers"),
        (name = "agents", description = "Owned CMA Agents and their durable history"),
        (name = "resource-access", description = "Current effective policy and owner-managed organization shares"),
        (name = "agent operations", description = "Input and control operations for owned CMA Agents"),
        (name = "agent usage", description = "Usage and resource readings for owned CMA Agents"),
        (name = "sandboxes", description = "Owned Sandbox lifecycle and event replay"),
        (name = "workspaces", description = "Owned Workspace lifecycle and hierarchy"),
        (name = "worktrees", description = "Owned Worktree hierarchy reads"),
        (name = "administration", description = "Capability-gated deployment operations"),
    )
)]
struct ApiDoc;

pub fn document() -> utoipa::openapi::OpenApi {
    let mut document = ApiDoc::openapi();
    document.merge(crate::responses::document());
    document.merge(crate::agui::document());
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
            operation.security = Some(if anonymous_path(path) {
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

fn anonymous_path(path: &str) -> bool {
    matches!(
        path,
        "/auth/signup-options"
            | "/api/v1/conformance"
            | "/api/v1/agui/capabilities"
            | "/api/v1/triggers/public/{public_id}"
            | "/api/v1/triggers/public/{public_id}/badge.svg"
            | "/api/v1/triggers/public/{public_id}/open/{placement_id}"
            | "/api/v1/triggers/public/{public_id}/entry-intent"
    )
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
mod tests;
