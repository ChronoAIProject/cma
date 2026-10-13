//! Apply human authentication to owner routes and capability auth to tool routes.
use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Router};

use crate::{
    activity::ActivityRecorder, auth::AuthState, connections::BrowserConnections,
    rate_limit::RateLimiters, store::MongoStores, REQUEST_BODY_LIMIT_BYTES,
};

pub(super) fn compose(
    resources: Router,
    connections: Arc<BrowserConnections>,
    auth: Arc<AuthState>,
    recorder: Arc<ActivityRecorder>,
    limits: Arc<RateLimiters>,
    stores: Arc<MongoStores>,
    tools: Router,
) -> Router {
    let public = crate::rest_router(resources)
        .merge(crate::connections::router(connections))
        .merge(crate::auth::router(auth.clone()))
        .layer(axum::middleware::from_fn_with_state(
            recorder,
            crate::activity::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            limits.clone(),
            crate::rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ));
    Router::new()
        .merge(public)
        .merge(tools)
        .merge(crate::health::router(stores))
        .layer(axum::middleware::from_fn_with_state(
            limits,
            crate::rate_limit::admit,
        ))
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES))
}
