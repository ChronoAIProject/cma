//! Assemble public interface contracts and apply their shared authentication.
use utoipa::OpenApi as _;

pub fn document() -> utoipa::openapi::OpenApi {
    let mut document = super::ApiDoc::openapi();
    document.merge(crate::rest::documents::document());
    document.merge(crate::rest::bindings::preparation::document());
    document.merge(crate::responses::document());
    document.merge(crate::agui::document());
    document.merge(crate::agent_v2::document());
    super::identity::secure_operations(&mut document);
    document
}
