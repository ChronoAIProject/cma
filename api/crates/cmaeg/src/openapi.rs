//! The code-derived v1 protocol document and stable snapshot export.
use crate::http;
use utoipa::OpenApi;

/// The versioned CMAEG management API document.
#[derive(Debug, OpenApi)]
#[openapi(
    info(title = "CMA Event Gateway", version = "1.0"),
    servers((url = "/v1")),
    modifiers(&ManagementSecurity, &ProtocolBase),
    components(schemas(crate::providers::EventContextSnapshot)),
    paths(
        http::management::create_channel,
        http::management::list_channels,
        http::management::permission_catalog,
        http::creation::recover_channel,
        http::management::get_channel,
        http::management::update_channel,
        http::management::delete_channel,
        http::management::test_channel,
        http::management::rekey_channel,
        http::callbacks::callback,
        http::events::receive_event,
        http::tools::reply_to_event,
        http::tools::update_message,
        http::tools::send_typing,
        http::tools::read_event_context,
    )
)]
pub struct ApiDoc;

struct ManagementSecurity;
impl utoipa::Modify for ManagementSecurity {
    fn modify(&self, document: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let components = document.components.get_or_insert_with(Default::default);
        for (name, description) in [
            ("creatorBearer", "NyxID creator bearer token"),
            ("serviceBearer", "CMA service token; rekey only"),
            ("agentBearer", "NyxID bot agent key; tools only"),
        ] {
            components.add_security_scheme(
                name,
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some(description))
                        .build(),
                ),
            );
        }
    }
}

struct ProtocolBase;
impl utoipa::Modify for ProtocolBase {
    fn modify(&self, document: &mut utoipa::openapi::OpenApi) {
        // OpenAPI paths are relative to the /v1 server; the handlers still use
        // their full Axum paths. Never publish a duplicated /v1/v1 base URL.
        document.paths.paths = std::mem::take(&mut document.paths.paths)
            .into_iter()
            .map(|(path, item)| {
                (
                    path.strip_prefix("/v1").expect("v1 operation").to_owned(),
                    item,
                )
            })
            .collect();
    }
}
pub fn snapshot() -> String {
    let mut document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI is serializable");
    sort(&mut document);
    format!("{}\n", serde_json::to_string_pretty(&document).unwrap())
}
fn sort(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let entries: std::collections::BTreeMap<_, _> =
                std::mem::take(object).into_iter().collect();
            for (key, mut value) in entries {
                sort(&mut value);
                object.insert(key, value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                sort(value);
            }
        }
        _ => {}
    }
}
