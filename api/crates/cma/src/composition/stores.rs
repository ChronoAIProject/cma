//! Durable store connection and deployment-specific writer policy.
use std::sync::Arc;

use crate::{config::Config, store::MongoStores};

pub(super) async fn connect(config: &Config) -> Result<Arc<MongoStores>, String> {
    let mut stores = MongoStores::connect_with_authority(
        &config.mongodb_uri,
        &config.mongodb_database,
        &config.deployment_id,
        &config.auth_encryption_key,
        config.activity_retention,
        (
            &config.runtime.sandbox_image,
            config.runtime.home_policy.as_ref(),
        ),
        config.mongodb_authority,
    )
    .await
    .map_err(|error| format!("durable stores are not usable: {error}"))?;
    stores.sandbox_model = Arc::new(
        (*stores.sandbox_model)
            .clone()
            .with_machine_runtime_version(config.runtime.machine_sandbox_image.clone()),
    );
    stores.sandbox_model = Arc::new(
        (*stores.sandbox_model)
            .clone()
            .prepare_bounded_admission(&config.bounded_admission_agents)
            .await?,
    );
    Ok(Arc::new(
        stores.with_writer_policy(config.task_experience.clone()),
    ))
}
