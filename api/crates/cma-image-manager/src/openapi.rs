//! Anonymous, code-derived discovery for the optional Image Manager service.
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(components(schemas(crate::http::shapes::Problem, crate::http::ImageScopeParam)))]
struct Errors;

pub fn document() -> Value {
    let mut generated = crate::http::api_router().split_for_parts().1;
    generated.merge(Errors::openapi());
    let mut document = serde_json::to_value(generated).expect("Image Manager OpenAPI serializes");
    document["info"] = json!({
        "title": "CMA Image Manager API", "version": env!("CARGO_PKG_VERSION"),
        "description": "Optional Image Manager service: owned image catalog, builds, immutable versions and sharing. \
          Authenticate with a NyxID user access token authorized for the cma-image-manager resource and required cma-im permissions, \
          or this service's same-origin browser session. Reads also require ownership, explicit sharing or public visibility; \
          control/build operations remain owner-only. Creation and build submission require Idempotency-Key; \
          edit, retirement, visibility and sharing require the current numeric revision in If-Match. \
          Lists contain at most 100 items and next_cursor; supply it as after. Requests are limited to 32 KiB. \
          This document describes implemented operations and does not assert that this optional service is deployed."
    });
    document["servers"] = json!([{
        "url": "/api/image-manager", "description": "Same-origin Image Manager proxy when the service is enabled"
    }]);
    document["components"]["securitySchemes"] = json!({
        "bearerAuth": {"type": "http", "scheme": "bearer", "description": "NyxID access token for the cma-image-manager resource"},
        "sessionCookie": {"type": "apiKey", "in": "cookie", "name": "__Host-cma-image-manager", "description": "Image Manager browser session; cookie mutations require the trusted same origin"}
    });
    document["tags"] = json!([
        {"name":"permissions", "description":"Current caller permissions and service roles"},
        {"name":"images", "description":"Accessible image catalog and owner-managed metadata/shares"},
        {"name":"builds", "description":"Owner-only asynchronous builds and bounded logs"},
        {"name":"versions", "description":"Immutable successful versions and authorized runtime resolution"}
    ]);
    for (path, item) in document["paths"]
        .as_object_mut()
        .expect("OpenAPI paths")
        .iter_mut()
    {
        for (method, operation) in item.as_object_mut().expect("path item") {
            if !["get", "post", "put", "patch", "delete"].contains(&method.as_str()) {
                continue;
            }
            operation["security"] = json!([{"bearerAuth":[]},{"sessionCookie":[]}]);
            let responses = operation["responses"]
                .as_object_mut()
                .expect("operation responses");
            for (status, description) in [
                ("401", "Missing, invalid or revoked caller authentication"),
                ("403", "Required Image Manager permission is absent"),
                ("503", "Service dependency unavailable"),
            ] {
                responses.insert(status.into(), problem_response(description));
            }
            // Resource access deliberately conceals foreign resources as absent.
            if path.contains('{') {
                operation["responses"]["404"] =
                    problem_response("Image, version or build is absent or inaccessible");
            }
            if ["post", "put", "patch", "delete"].contains(&method.as_str()) {
                operation["responses"]["400"] =
                    problem_response("Invalid request values or required concurrency header");
                operation["responses"]["409"] =
                    problem_response("The revision, idempotency key or current resource conflicts");
            }
            if operation["operationId"] == "image_manager_logs" {
                operation["responses"]["409"] =
                    problem_response("Build logs are not available for the current build state");
            }
            if operation
                .get("parameters")
                .and_then(Value::as_array)
                .is_some_and(|parameters| {
                    parameters
                        .iter()
                        .any(|parameter| parameter["in"] == "query")
                })
            {
                operation["responses"]["400"] = json!({"description":"Invalid pagination or scope query", "content":{"text/plain":{"schema":{"type":"string"}}}});
            }
            if operation.get("requestBody").is_some() {
                for (status, description) in [("400", "Malformed JSON or invalid request values; extractor failures use text/plain"),
                    ("413", "Request exceeds 32 KiB"), ("415", "JSON content type required"), ("422", "JSON does not match the request fields")]
                {
                    let existing = operation["responses"].as_object_mut().unwrap().entry(status).or_insert_with(|| json!({"description":description,"content":{}}));
                    existing["description"] = json!(description);
                    existing["content"]["text/plain"] = json!({"schema":{"type":"string"}});
                }
            }
        }
    }
    document
}

fn problem_response(description: &str) -> Value {
    json!({"description":description,"content":{"application/problem+json":{"schema":{"$ref":"#/components/schemas/Problem"}}}})
}

async fn read() -> Json<Value> {
    Json(document())
}

/// Merge after browser protection so discovery needs no user credential or DB IO.
pub fn router() -> Router {
    Router::new()
        .route("/openapi.json", get(read))
        .route("/image-manager/openapi.json", get(read))
}

#[cfg(test)]
mod tests;
