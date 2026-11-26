use std::sync::Arc;

use crate::{
    config::Config,
    store::{creation_outcomes::CreationOutcomes, MongoStores},
};
use axum::Router;
use layer1_public::rest::creation_outcomes::{self, Backend};

pub(super) async fn open(
    config: &Config,
    stores: &MongoStores,
) -> Result<Option<Arc<CreationOutcomes>>, String> {
    match &config.creation_outcomes {
        Some(policy) => stores
            .creation_outcomes(policy.clone())
            .await
            .map(Arc::new)
            .map(Some)
            .map_err(|error| error.to_string()),
        None => Ok(None),
    }
}

pub(super) fn routes(resources: Router, backend: Option<&CreationOutcomes>) -> Router {
    let Some(backend) = backend else {
        return resources;
    };
    let backend: Arc<dyn Backend> = Arc::new(backend.clone());
    resources
        .merge(creation_outcomes::router(backend.clone()))
        .layer(axum::Extension(backend))
}
