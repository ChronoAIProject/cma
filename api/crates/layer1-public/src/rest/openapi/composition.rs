use utoipa::OpenApi;

pub fn document() -> utoipa::openapi::OpenApi {
    let mut document = super::ApiDoc::openapi();
    document.merge(super::super::documents::document());
    document.merge(super::super::message_feedback::document());
    document.merge(crate::responses::document());
    document.merge(crate::agui::document());
    document.merge(crate::agent_v2::document());
    super::identity::secure_operations(&mut document);
    document
}
