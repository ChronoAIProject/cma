//! chrono-bot control plane. Wires Layer 1 interfaces onto the Layer 2 core,
//! which drives kernels through Layer 3 adapters.
//!
//! This process never executes agent code. It serves public traffic, keeps one
//! rendered screen per live session, and relays terminals to and from sandboxed
//! runtimes that run elsewhere.

mod auth;
mod config;
mod screen;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{routing::get, Json, Router};
use layer1_public::{
    a2a::A2aInterface, agui::AguiInterface, mcp::McpInterface, pty::PtyInterface,
    rest::RestInterface, InterfaceProfile,
};
use layer2_core::{KernelId, ScreenModel, SessionService, TerminalGeometry};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    sandbox::SandboxClient,
    KernelAdapter,
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::auth::AuthState;
use crate::config::Config;
use crate::screen::VtScreen;

fn interfaces() -> Vec<Box<dyn InterfaceProfile>> {
    vec![
        Box::new(AguiInterface),
        Box::new(RestInterface),
        Box::new(PtyInterface),
        Box::new(McpInterface),
        Box::new(A2aInterface),
    ]
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            // Configuration failures are fatal and must say exactly what is
            // missing; a control plane that starts half-configured is worse
            // than one that refuses to start.
            tracing::error!(error = %e, "configuration is not usable");
            std::process::exit(2);
        }
    };
    tracing::info!(
        bind = %config.bind,
        public_url = %config.public_url,
        sandbox = %config.sandbox_base_url,
        sandbox_image = %config.sandbox_image,
        auth = config.auth_mode_name(),
        "configuration loaded"
    );

    for iface in interfaces() {
        tracing::info!(
            interface = iface.interface_id(),
            operations = iface.operations().len(),
            "interface projected"
        );
    }

    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    // The only place a concrete runtime implementation is named. Everything
    // upstream of here selects by opaque identifier.
    adapters.insert(
        KernelId::new("codex"),
        Arc::new(CodexAdapter::new(
            SandboxClient::new(
                config.sandbox_base_url.clone(),
                config.sandbox_api_key.clone(),
            ),
            CodexSandboxSpec {
                image: config.sandbox_image.clone(),
                ..CodexSandboxSpec::default()
            },
        )),
    );

    let screen_factory: layer2_core::ScreenFactory = Arc::new(|geometry: TerminalGeometry| {
        Box::new(VtScreen::new(geometry)) as Box<dyn ScreenModel>
    });
    let service = SessionService::new(adapters, screen_factory);

    let auth_state = AuthState::new(config.auth_mode.clone());
    if !auth_state.requires_credential() {
        tracing::warn!(
            "running with a trusted fixed subject; every request is attributed \
             to the same principal. This must never be used outside local development."
        );
    }

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(config.cors_origins.clone()))
        .allow_credentials(true)
        .allow_headers([axum::http::header::CONTENT_TYPE])
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::DELETE,
        ]);

    let app = Router::new()
        .route("/livez", get(|| async { "ok" }))
        .route("/ready", get(ready))
        .merge(layer1_public::rest::router(Arc::clone(&service)))
        .merge(layer1_public::pty::router(Arc::clone(&service)))
        .merge(auth::router(Arc::clone(&auth_state)))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&auth_state),
            auth::attach_subject,
        ))
        .layer(cors);

    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(bind = %config.bind, error = %e, "cannot bind");
            std::process::exit(2);
        }
    };
    tracing::info!(bind = %config.bind, "control plane listening");

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(error = %e, "server stopped with an error");
        std::process::exit(1);
    }
}

async fn ready() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ready" }))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("ctrl-c handler installs");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler installs")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received; draining");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_interface_has_a_unique_id() {
        let ids: Vec<_> = interfaces().iter().map(|i| i.interface_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "interface ids must be unique");
    }

    #[test]
    fn every_interface_projects_at_least_one_operation() {
        for iface in interfaces() {
            assert!(
                !iface.operations().is_empty(),
                "{} projects no operations",
                iface.interface_id()
            );
        }
    }
}
