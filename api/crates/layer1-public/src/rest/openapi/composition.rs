//! Assemble public interface contracts and apply their shared authentication.
use utoipa::OpenApi as _;
#[path = "capacity.rs"]
mod capacity;

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        super::super::agent_original_request::read,
        super::super::agent_reply_preview::read,
    ),
    components(schemas(
        super::super::agent_original_request::OriginalRequestResponse,
        super::super::agent_original_request::OriginalRequestView,
        super::super::agent_original_request::OriginalRequestSource,
        super::super::agent_reply_preview::ReplyPreviewResponse,
        super::super::agent_reply_preview::AssistantReplyPreviewView,
    ))
)]
struct AgentHistoryReads;

pub fn document() -> utoipa::openapi::OpenApi {
    let mut document = super::ApiDoc::openapi();
    document.merge(AgentHistoryReads::openapi());
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert("x-cma-task-repository".into(), serde_json::json!(true));
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert("x-cma-task-templates-v1".into(), serde_json::json!(true));
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert("x-cma-task-templates-v2".into(), serde_json::json!(true));
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert("x-cma-agent-title-mode".into(), serde_json::json!(true));
    document.merge(super::super::agent_capacity_recovery::document());
    document.merge(super::super::agent_retained_output::document());
    document.merge(super::super::resource_revocations::document());
    document.merge(super::super::admin::admission::document());
    document.merge(super::admin_runtime::AdminRuntimeApi::openapi());
    document.merge(super::super::admin::spend::document());
    document.merge(super::super::account_deletion::document());
    document.merge(super::super::legal_consent::document());
    document.merge(super::super::privacy_export::document());
    document.merge(super::super::browser_telemetry::document());
    document.merge(super::super::documents::document());
    document.merge(super::super::desktop_creation::document());
    document.merge(super::super::agent_filesystem::browser::document());
    document.merge(super::super::agent_filesystem::browser_control::document());
    document.merge(super::super::answer_deliverables::AnswerDeliverablesApi::openapi());
    document.merge(super::super::message_feedback::document());
    document.merge(crate::rest::sandbox_capacity::document());
    document.merge(crate::rest::sandbox_usage::document());
    document.merge(crate::rest::sandbox_metrics::document());
    document.merge(crate::rest::sandbox_retained_recovery::document());
    document.merge(crate::rest::runtime_canary::document());
    document.merge(crate::rest::retained_runtime_transition::document());
    document.merge(crate::rest::sandbox_commands::document());
    document.merge(crate::rest::bindings::preparation::document());
    document.merge(crate::rest::bindings::inspection::document());
    document.merge(crate::rest::bindings::reset::document());
    document.merge(crate::rest::binding_media::document());
    document.merge(crate::rest::worker_credentials::document());
    document.merge(crate::rest::agent_provider_associations::document());
    document.merge(crate::responses::document());
    document.merge(crate::agui::document());
    document.merge(crate::agent_v2::document());
    super::identity::secure_operations(&mut document);
    capacity::describe(&mut document);
    document
}
