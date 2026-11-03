use utoipa::OpenApi;

pub(super) fn compose(mut document: utoipa::openapi::OpenApi) -> utoipa::openapi::OpenApi {
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert("x-cma-task-repository".into(), serde_json::json!(true));
    document.merge(super::super::documents::document());
    document.merge(super::super::answer_deliverables::AnswerDeliverablesApi::openapi());
    document.merge(crate::responses::document());
    document.merge(crate::agui::document());
    document.merge(crate::agent_v2::document());
    super::identity::secure_operations(&mut document);
    document
}
