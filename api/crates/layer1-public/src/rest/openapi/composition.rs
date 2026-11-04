//! Assemble public interface contracts and apply their shared authentication.
use utoipa::OpenApi as _;

pub fn document() -> utoipa::openapi::OpenApi {
    let mut document = super::ApiDoc::openapi();
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert("x-cma-task-repository".into(), serde_json::json!(true));
    document.merge(super::super::documents::document());
    document.merge(super::super::desktop_creation::document());
    document.merge(super::super::agent_filesystem::browser::document());
    document.merge(super::super::agent_filesystem::browser_control::document());
    document.merge(super::super::answer_deliverables::AnswerDeliverablesApi::openapi());
    document.merge(super::super::message_feedback::document());
    document.merge(crate::rest::sandbox_usage::document());
    document.merge(crate::rest::bindings::preparation::document());
    document.merge(crate::rest::bindings::inspection::document());
    document.merge(crate::rest::binding_media::document());
    document.merge(crate::rest::worker_credentials::document());
    document.merge(crate::rest::agent_provider_associations::document());
    document.merge(crate::responses::document());
    document.merge(crate::agui::document());
    document.merge(crate::agent_v2::document());
    super::identity::secure_operations(&mut document);
    document
}
