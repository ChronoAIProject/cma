//! Projection of authoritative owner quota observations.
use super::*;

#[utoipa::path(get, path="/api/v1/usage/documents", tag="usage", responses(
    (status=200, body=DocumentUsage),
    (status=401, description="Authenticated subject required", body=super::super::ProblemDocument, content_type="application/problem+json"),
    (status=404, description="Document principal authority required", body=super::super::ProblemDocument, content_type="application/problem+json"),
    (status=503, description="Document quota ledger unavailable", body=super::super::ProblemDocument, content_type="application/problem+json")
))]
pub(crate) async fn read(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<layer2_core::document_artifacts::DocumentUsage>, DocumentProblem> {
    Ok(Json(service.usage(&caller).await?))
}
