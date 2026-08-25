//! Generated OpenAPI document and its anonymous discovery route.

use std::sync::Arc;

use axum::{Extension, Json, Router};
use utoipa::{openapi, PartialSchema, ToSchema};
use utoipa_axum::{router::OpenApiRouter, routes};

use super::{ProblemDocument, SERVICE_VERSION};

pub(super) const DOCUMENT_PATH: &str = "/openapi.json";

/// The stable top-level shape returned by the OpenAPI discovery route.
#[allow(dead_code)]
#[derive(ToSchema)]
struct OpenApiDocument {
    openapi: String,
    info: OpenApiDocumentInfo,
    #[schema(value_type = Object)]
    paths: serde_json::Value,
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct OpenApiDocumentInfo {
    title: String,
    version: String,
}

/// Serve the exact document collected while registering the REST handlers.
#[utoipa::path(
    get,
    path = "/openapi.json",
    tag = "discovery",
    responses((status = 200, description = "OpenAPI 3.1 contract", body = OpenApiDocument, content_type = "application/json")),
    security(())
)]
async fn document(Extension(document): Extension<Arc<openapi::OpenApi>>) -> Json<openapi::OpenApi> {
    Json((*document).clone())
}

/// Add the discovery route, shared errors, authentication, and response headers
/// after every handler has contributed its paths and concrete schemas.
pub(super) fn finish(mut documented: OpenApiRouter) -> Router {
    documented = documented.routes(routes!(document));
    finish_document(documented.get_openapi_mut());
    let document = Arc::new(documented.get_openapi().clone());
    let router: Router = documented.into();
    router.layer(Extension(document))
}

fn finish_document(document: &mut openapi::OpenApi) {
    document.info.title = "CMA REST API".to_string();
    document.info.version = SERVICE_VERSION.to_string();
    document.info.description = Some(
        "The complete CMA REST control surface. Refusals use RFC 9457 problem details.".to_string(),
    );

    let components = document
        .components
        .get_or_insert_with(openapi::Components::new);
    components.schemas.insert(
        ProblemDocument::name().to_string(),
        ProblemDocument::schema(),
    );
    components.add_security_scheme(
        "bearerAuth",
        openapi::security::SecurityScheme::Http(
            openapi::security::HttpBuilder::new()
                .scheme(openapi::security::HttpAuthScheme::Bearer)
                .build(),
        ),
    );
    components.add_security_scheme(
        "sessionCookie",
        openapi::security::SecurityScheme::ApiKey(openapi::security::ApiKey::Cookie(
            openapi::security::ApiKeyValue::with_description(
                "cma_session",
                "CMA browser session cookie",
            ),
        )),
    );

    let request_id = openapi::header::HeaderBuilder::new()
        .description(Some(
            "Correlation identifier echoed from the request or minted by CMA",
        ))
        .build();
    let link = openapi::header::HeaderBuilder::new()
        .description(Some(
            "RFC 8288 links to the REST profile, service description, and any next page",
        ))
        .build();
    let request_id_parameter = openapi::path::ParameterBuilder::new()
        .name("X-Request-Id")
        .parameter_in(openapi::path::ParameterIn::Header)
        .required(openapi::Required::False)
        .description(Some(
            "Optional non-empty text caller correlation identifier; usable values are echoed unchanged",
        ))
        .schema(Some(openapi::Object::with_type(openapi::Type::String)))
        .build();
    let problem = openapi::response::ResponseBuilder::new()
        .description("RFC 9457 problem details")
        .header("X-Request-Id", request_id.clone())
        .header("Link", link.clone())
        .content(
            "application/problem+json",
            openapi::Content::new(Some(
                openapi::Ref::from_schema_name(ProblemDocument::name()),
            )),
        )
        .build();
    components
        .responses
        .insert("Problem".to_string(), problem.into());

    // Utoipa's shorthand couples request-body optionality to `Option<T>`,
    // which also makes the JSON value nullable. Runtime accepts an omitted or
    // empty ticket body but deliberately rejects an explicit JSON `null`, so
    // express those two dimensions independently in the finished document.
    let ticket_body = document
        .paths
        .paths
        .get_mut("/api/v1/sessions/{id}/terminal-ticket")
        .and_then(|item| item.post.as_mut())
        .and_then(|operation| operation.request_body.as_mut())
        .expect("the terminal ticket route declares a request body");
    ticket_body.required = Some(openapi::Required::False);
    ticket_body
        .content
        .get_mut("application/json")
        .expect("the terminal ticket body is JSON")
        .schema = Some(openapi::Ref::from_schema_name("TicketRequest").into());

    // Replacement also accepts the complete view returned by GET. The custom
    // extractor validates and removes its server-owned fields before decoding
    // the writable projection, while create remains ProfileRequest-only.
    let profile_replacement = document
        .paths
        .paths
        .get_mut("/api/v1/environment-profiles/{id}")
        .and_then(|item| item.put.as_mut())
        .and_then(|operation| operation.request_body.as_mut())
        .and_then(|body| body.content.get_mut("application/json"))
        .expect("the profile replacement body is JSON");
    profile_replacement.schema = Some(
        openapi::schema::OneOfBuilder::new()
            .item(openapi::Ref::from_schema_name("ProfileRequest"))
            .item(openapi::Ref::from_schema_name("ProfileView"))
            .into(),
    );

    for (path, item) in &mut document.paths.paths {
        for (method, operation) in [
            ("get", item.get.as_mut()),
            ("put", item.put.as_mut()),
            ("post", item.post.as_mut()),
            ("delete", item.delete.as_mut()),
            ("options", item.options.as_mut()),
            ("head", item.head.as_mut()),
            ("patch", item.patch.as_mut()),
            ("trace", item.trace.as_mut()),
        ]
        .into_iter()
        .filter_map(|(method, operation)| operation.map(|operation| (method, operation)))
        {
            operation.operation_id = Some(operation_id(method, path));
            operation
                .parameters
                .get_or_insert_default()
                .push(request_id_parameter.clone());
            operation
                .responses
                .responses
                .entry("default".to_string())
                .or_insert_with(|| openapi::Ref::from_response_name("Problem").into());
            for response in operation.responses.responses.values_mut() {
                if let openapi::RefOr::T(response) = response {
                    response
                        .headers
                        .entry("X-Request-Id".to_string())
                        .or_insert_with(|| request_id.clone());
                    response
                        .headers
                        .entry("Link".to_string())
                        .or_insert_with(|| link.clone());
                }
            }

            if path == DOCUMENT_PATH || path == "/api/v1/conformance" {
                operation.security = Some(Vec::new());
            } else {
                operation.security = Some(vec![
                    openapi::security::SecurityRequirement::new(
                        "bearerAuth",
                        std::iter::empty::<String>(),
                    ),
                    openapi::security::SecurityRequirement::new(
                        "sessionCookie",
                        std::iter::empty::<String>(),
                    ),
                ]);
            }
        }
    }
}

fn operation_id(method: &str, path: &str) -> String {
    let mut id = method.to_string();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        id.push('_');
        if segment.starts_with('{') && segment.ends_with('}') {
            id.push_str("by_");
        }
        let segment = segment.trim_matches(['{', '}']);
        let mut previous_was_separator = false;
        for character in segment.chars() {
            if character.is_ascii_alphanumeric() || character == '_' {
                id.push(character.to_ascii_lowercase());
                previous_was_separator = false;
            } else if !previous_was_separator {
                id.push('_');
                previous_was_separator = true;
            }
        }
        if id.ends_with('_') {
            id.pop();
        }
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use utoipa::OpenApi;

    #[test]
    fn generated_documents_are_openapi_3_1() {
        let document = <ApiVersion as OpenApi>::openapi();
        assert!(document.openapi == openapi::OpenApiVersion::Version31);
    }

    #[test]
    fn operation_identifiers_are_stable_sdk_names() {
        assert_eq!(
            operation_id("get", "/api/v1/sessions/{id}"),
            "get_api_v1_sessions_by_id"
        );
        assert_eq!(operation_id("get", "/openapi.json"), "get_openapi_json");
    }

    #[derive(OpenApi)]
    struct ApiVersion;
}
