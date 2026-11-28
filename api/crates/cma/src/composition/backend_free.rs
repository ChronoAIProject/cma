//! HTTP account and settings surfaces with no Sandbox execution composition.
use std::{net::SocketAddr, sync::Arc};

use axum::{extract::DefaultBodyLimit, Router};
use layer2_core::{ActivityService, PlatformSettingsService, UserDirectory};

use crate::{
    auth::{self, account_standing::ErasingSnapshot},
    config::BackendFreeConfig,
    rate_limit::RateLimiters,
    store::mongo::MongoAccountStores,
    telemetry, REQUEST_BODY_LIMIT_BYTES,
};

mod refusals;

pub(crate) async fn serve(config: BackendFreeConfig) -> Result<(), String> {
    if std::env::args().nth(1).as_deref() == Some("--agent-manager-drain") {
        return Err("Agent Manager drain requires a Sandbox execution backend".into());
    }
    let recorder = telemetry::install()?;
    let _metrics_server =
        telemetry::server::Server::start(config.server.metrics_bind, recorder, Default::default())
            .await?;
    let (router, standing) = compose(&config).await?;
    let app = crate::with_request_context(crate::with_cors(router, config.server.cors_origins));
    let listener = tokio::net::TcpListener::bind(config.server.bind)
        .await
        .map_err(|error| format!("cannot bind {}: {error}", config.server.bind))?;
    tracing::info!(bind = %config.server.bind, "CMA account control plane listening");
    let result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(crate::shutdown::shutdown_signal())
    .await
    .map_err(|error| format!("HTTP server failed: {error}"));
    standing.abort();
    let _ = standing.await;
    result
}

pub(crate) async fn compose(
    config: &BackendFreeConfig,
) -> Result<(Router, tokio::task::JoinHandle<()>), String> {
    let stores = MongoAccountStores::connect(config)
        .await
        .map_err(|error| format!("durable account stores are not usable: {error}"))?;
    let activity = ActivityService::new(stores.activity.clone());
    let users = UserDirectory::new(stores.users.clone());
    let settings = Arc::new(
        PlatformSettingsService::new(stores.settings.clone(), activity.clone())
            .with_model_budget_writes_enabled(config.model_budget_writes)
            .with_model_request_budget_writes_enabled(config.model_request_budget_writes)
            .with_platform_spend_writes_enabled(config.platform_spend_writes),
    );
    stores
        .settings
        .read_for_startup()
        .await
        .map_err(|error| error.to_string())?;
    let standing = Arc::new(ErasingSnapshot::new(stores.accounts.clone()));
    let auth = super::credentials::account_authentication(
        config,
        &stores,
        &users,
        &activity,
        settings.clone(),
    )?
    .with_account_standing(standing.clone());
    let limits = RateLimiters::from_config(
        config.rate_limits,
        config.trusted_proxies.clone(),
        stores.request_limits.clone(),
    );
    let resources = layer1_public::rest::admin::platform_settings_router(settings)
        .merge(layer1_public::rest::personal_agent_inventory_router(
            stores.clone(),
        ))
        .merge(refusals::router(
            stores.clone(),
            super::desktop::origins(&config.server.public_url, &config.server.cors_origins),
        ));
    let public = crate::rest_router(resources)
        .merge(auth::account_router(auth.clone()))
        .layer(axum::middleware::from_fn_with_state(
            limits.clone(),
            crate::rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            auth::attach_subject,
        ));
    let app = public
        .merge(crate::health::backend_free_router(stores))
        .layer(axum::middleware::from_fn_with_state(
            limits,
            crate::rate_limit::admit,
        ))
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES));
    Ok((app, standing.start().await))
}
