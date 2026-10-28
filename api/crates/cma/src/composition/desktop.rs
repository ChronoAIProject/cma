//! Compose the public desktop transport with its owner-authorized domain service.
use std::sync::Arc;

pub(crate) fn router(
    reads: Arc<layer2_core::sandbox::AgentReadService>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    public_url: &str,
) -> axum::Router {
    let origin = reqwest::Url::parse(public_url)
        .expect("configured public URL is valid")
        .origin()
        .ascii_serialization();
    layer1_public::rest::desktop_router(
        Arc::new(layer2_core::sandbox::DesktopService::new(reads, adapter)),
        origin,
    )
}
