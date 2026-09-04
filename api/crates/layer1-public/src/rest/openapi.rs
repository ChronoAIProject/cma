//! OpenAPI generation for the currently composed REST boundary.

use utoipa::OpenApi;

use super::openapi_shapes::ConformanceDocument;
use super::problems::ProblemDocument;

#[derive(OpenApi)]
#[openapi(
    paths(super::conformance),
    components(schemas(ConformanceDocument, ProblemDocument)),
    tags((name = "discovery", description = "Anonymous deployment discovery"))
)]
struct ApiDoc;

pub fn document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
