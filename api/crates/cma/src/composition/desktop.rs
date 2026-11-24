//! Compose the public desktop transport with its owner-authorized domain service.
use layer2_core::sandbox::DesktopAdmission;
use std::sync::Arc;

pub(crate) fn router(
    reads: Arc<layer2_core::sandbox::AgentReadService>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    config: &crate::config::Config,
) -> axum::Router {
    layer1_public::rest::desktop_router_with_origins(
        Arc::new(
            layer2_core::sandbox::DesktopService::new(reads, adapter)
                .with_admission(if config.agent_manager_replicas.is_some() {
                    DesktopAdmission::Protected
                } else {
                    DesktopAdmission::Disabled
                })
                .with_runtime_migration_tracking(config.runtime.migration.enabled),
        ),
        origins(&config.public_url, &config.cors_origins),
    )
}

pub(super) fn origins(public_url: &str, cors_origins: &[axum::http::HeaderValue]) -> Vec<String> {
    let origin = reqwest::Url::parse(public_url)
        .expect("configured public URL is valid")
        .origin()
        .ascii_serialization();
    let mut origins = vec![origin];
    for value in cors_origins {
        let origin = value
            .to_str()
            .expect("configured CORS origin is valid")
            .to_owned();
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    origins
}
