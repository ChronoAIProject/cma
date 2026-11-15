//! Construct only the independently authenticated, finite customer read router.
use std::sync::Arc;

use crate::{
    auth::{AuthMode, AuthState},
    config::Config,
};

pub(crate) async fn compose(config: &Config) -> Result<axum::Router, String> {
    let client = client(config).await?;
    let auth = authentication(config, &client).await?;
    let hierarchy = crate::store::read_only::ReadOnlyHierarchyStore::open(
        client.clone(),
        client.database(&config.mongodb_database),
    )
    .await
    .map_err(|_| "inert hierarchy store is unavailable")?;
    let router = layer1_public::rest::read_only_hierarchy_router(
        layer2_core::sandbox::HierarchyReadService::new(Arc::new(hierarchy)),
    );
    Ok(router.layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::read_only::attach,
    )))
}

pub(crate) async fn verify(
    config: &Config,
    headers: &axum::http::HeaderMap,
) -> Result<crate::auth::read_only::ReadProof, String> {
    let client = client(config).await?;
    let auth = authentication(config, &client).await?;
    crate::auth::read_only::authenticate(auth, headers)
        .await
        .map_err(|_| "fresh existing-session authority is unavailable".into())
}

async fn client(config: &Config) -> Result<mongodb::Client, String> {
    if !matches!(config.auth_mode, AuthMode::Provider { .. }) {
        return Err("inert customer reads require provider authentication".into());
    }
    let mut options = mongodb::options::ClientOptions::parse(&config.mongodb_uri)
        .await
        .map_err(|_| "inert read database configuration is unavailable")?;
    options.selection_criteria = Some(mongodb::options::SelectionCriteria::ReadPreference(
        mongodb::options::ReadPreference::Primary,
    ));
    options.read_concern = Some(mongodb::options::ReadConcern::majority());
    options.server_selection_timeout = Some(std::time::Duration::from_secs(10));
    options.connect_timeout = Some(std::time::Duration::from_secs(10));
    mongodb::Client::with_options(options)
        .map_err(|_| "inert read database client is unavailable".into())
}

async fn authentication(
    config: &Config,
    client: &mongodb::Client,
) -> Result<Arc<AuthState>, String> {
    let auth_store = crate::store::auth::read_only::open(
        client,
        &config.mongodb_database,
        &config.deployment_id,
        &config.auth_encryption_key,
    )
    .await
    .map_err(|_| "inert credential store is unavailable")?;
    let auth = AuthState::with_store_and_nyxid_public_url(
        config.auth_mode.clone(),
        None,
        auth_store,
        config.nyxid_public_url.clone(),
    )
    .with_oauth_api_resource(config.nyxid_oauth_api_resource.clone());
    Ok(auth)
}
