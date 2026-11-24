use axum::{
    extract::OriginalUri,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use layer1_public::rest::HttpProblem;

const CAPABILITY: &str = "/api/v1/ui/image-manager-retirement";

pub(crate) fn mount(app: Router, enabled: bool) -> Router {
    let mut retirement = Router::new().route(
        CAPABILITY,
        get(move |OriginalUri(uri): OriginalUri| async move {
            let response = if uri.query() == Some("edge") {
                // Nginx authorization subrequests use 403 to refuse the old upstream.
                if enabled {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::NO_CONTENT
                }
                .into_response()
            } else {
                Json(enabled).into_response()
            };
            no_store(response)
        }),
    );
    if enabled {
        tracing::info!("Image Manager retirement enabled");
        retirement = retirement
            .route("/api/image-manager", any(not_found))
            .route("/api/image-manager/", any(not_found))
            .route("/api/image-manager/{*path}", any(not_found))
            .route("/image-manager/openapi.json", any(not_found));
    }
    retirement
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
