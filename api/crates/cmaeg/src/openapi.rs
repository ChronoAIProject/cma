//! The code-derived v1 protocol document and stable snapshot export.
use crate::http;
use utoipa::OpenApi;

/// The versioned CMAEG management API document.
#[derive(Debug, OpenApi)]
#[openapi(
    info(title = "CMA Event Gateway", version = "1.0"),
    servers((url = "/v1")),
    modifiers(&ManagementSecurity, &ProfileCompatibility, &ProtocolBase),
    // Query parameter schemas are referenced, not collected, so they are
    // registered here.
    components(schemas(
        crate::providers::EventContextSnapshot,
        cmaeg_core::channel::ProviderTarget,
        crate::tools::AttachmentViewKind,
        cmaeg_core::thread_lifecycle::ThreadListView,
        cmaeg_core::seal::HistoryCursor,
        cmaeg_core::message::NativeMediaUpdate,
        cmaeg_core::message::NativeMediaReplacement,
        crate::native_media::Capabilities,
    )),
    paths(
        crate::github::http::capabilities,
        crate::github::http::create_app,
        crate::github::http::list_apps,
        crate::github::http::read_app,
        crate::github::http::update_app,
        crate::github::http::verify_app,
        crate::github::http::retire_app,
        crate::github::http::webhook_secret,
        crate::github::http::bind,
        crate::github::http::binding,
        crate::github::http::verify_binding,
        crate::github::http::webhook,
        crate::github::http::receipt,
        crate::github::http::task,
        crate::github::http::lookup,
        http::activity::latest,
        http::activity::stream,
        http::activity_history::history,
        http::activity_history::details,
        http::media_responses::list,
        http::outbound::prepare,
        http::outbound::read,
        http::outbound::commit,
        http::outbound::cancel,
        http::connections::capabilities,
        http::connections::create,
        http::connections::list,
        http::relay_resource_connections::list,
        http::connections::read,
        http::connections::cancel,
        http::management::create_channel,
        http::management::list_channels,
        http::management::permission_catalog,
        http::creation::recover_channel,
        http::creation::cancel_create_capability,
        http::creation::cancel_create,
        http::record_management::read,
        http::record_management::retire,
        http::management::get_channel,
        http::partitions::inspect,
        http::profile_revisions::baseline,
        http::profile_revisions::publish,
        http::partition_resets::reset,
        http::management::update_channel,
        http::management::delete_channel,
        http::management::test_channel,
        http::channel_nodes::test_source,
        http::channel_nodes::test_consumer,
        http::channel_nodes::update_routing,
        http::management::rekey_channel,
        http::callbacks::callback,
        http::telegram::callback,
        http::native_lark::callback,
        http::whatsapp::challenge,
        http::whatsapp::callback,
        http::events::receive_event,
        http::event_attestation::attest_event,
        http::tools::reply_to_event,
        http::side_messages::capability,
        http::side_messages::send,
        http::form_preparation::prepare_form_reply,
        http::form_preparation::read_form_preparation,
        http::tools::update_message,
        http::history::read_event_history,
        http::threads::list_event_threads,
        http::threads::create_event_thread,
        http::threads::manage_thread,
        http::message_effects::read_message_capabilities,
        http::message_effects::delete_message,
        http::message_effects::set_message_reaction,
        http::tools::send_typing,
        http::tools::read_event_context,
        http::tools::verify_event_context,
        http::tools::read_event_attachment,
        http::media::download_media,
        http::reply_targets::create_reply_target,
        http::reply_targets::reply_to_target,
        http::reply_targets::reply_accepted_answer,
        http::reply_targets::read_accepted_answer_receipt,
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
            (
                "mediaBearer",
                "Single-use media link capability minted by readEventAttachment view download; Authorization header only",
            ),
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

// Keep the original v1 schema name available to generated clients.
struct ProfileCompatibility;
impl utoipa::Modify for ProfileCompatibility {
    fn modify(&self, document: &mut utoipa::openapi::OpenApi) {
        let components = document.components.as_mut().expect("protocol schemas");
        let profile = components
            .schemas
            .get("EventWorkerProfile")
            .expect("profile schema")
            .clone();
        components
            .schemas
            .insert("AgentProfileSnapshot".into(), profile);
    }
}
