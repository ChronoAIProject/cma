//! Mount public authentication, console and request audit at ingress.
use axum::{middleware::from_fn_with_state, Router};
use std::sync::Arc;

pub(super) fn mount(
    mut router: Router,
    browser: Option<cma_service_auth::NyxIdAuth>,
    console: Option<Router>,
    request_audit: Option<Arc<crate::request_audit::Writer>>,
) -> Router {
    if let Some(browser) = browser {
        router = router.merge(cma_service_auth::session::router(browser));
    }
    // The console keeps its own OAuth session, CSRF and owner checks.
    let router = router.merge(console.unwrap_or_default());
    match request_audit {
        Some(writer) => router.layer(from_fn_with_state(writer, crate::request_audit::audit)),
        None => router,
    }
}
