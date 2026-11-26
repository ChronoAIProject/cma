use axum::{
    extract::OriginalUri,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use layer1_public::rest::HttpProblem;

const CAPABILITY: &str = "/api/v1/ui/image-manager-retirement";

pub(crate) fn mount(app: Router) -> Router {
    Router::new()
        .route(
            CAPABILITY,
            get(|OriginalUri(uri): OriginalUri| async move {
                // Older edge and browser builds still read this capability.
                let response = if uri.query() == Some("edge") {
                    StatusCode::FORBIDDEN.into_response()
                } else {
                    Json(true).into_response()
                };
                no_store(response)
            }),
        )
        .route("/api/image-manager", any(not_found))
        .route("/api/image-manager/", any(not_found))
        .route("/api/image-manager/{*path}", any(not_found))
        .route("/image-manager/openapi.json", any(not_found))
        .route_layer(axum::middleware::from_fn(crate::rest_links))
        .merge(app)
}

async fn not_found() -> Response {
    no_store(HttpProblem::NotFound.into_response())
}

fn no_store(response: Response) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], response).into_response()
}

#[cfg(test)]
mod tests;
