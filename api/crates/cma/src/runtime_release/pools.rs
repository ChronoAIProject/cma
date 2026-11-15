//! Read-only startup checks: no readiness repair, resize or original-UID replacement.
use super::selection::target_matches;
use crate::{
    config::Config,
    release_authority::{ReleaseHead, ReleaseTarget},
    warm_pools::PoolTarget,
};
use layer3_adapter::agent_sandbox::{AgentSandboxClient, NativePoolControl};
use mongodb::{bson::doc, Database};

pub(super) async fn verify_selected(config: &Config, head: &ReleaseHead) -> Result<(), String> {
    let client = super::connect(config).await?;
    let database = client.database(&config.mongodb_database);
    super::migration::verify_floor(&database).await?;
    let settings = config
        .runtime
        .agent_sandbox
        .clone()
        .ok_or("native pool settings absent")?;
    let connection = layer3_adapter::agent_sandbox::resolve_kubernetes_connection(
        config.runtime.agent_sandbox_connection.as_ref(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let provider = AgentSandboxClient::new(connection.client, settings, &connection.authority)
        .map_err(|error| error.to_string())?;
    let inventory = NativePoolControl::inventory(provider.warm_pools())
        .await
        .map_err(|error| error.to_string())?;
    for pair in &head.protected_pairs {
        for (selected, machine) in std::iter::once((&pair.agent, false))
            .chain(pair.machine.iter().map(|target| (target, true)))
        {
            let target = read(&database, &head.deployment, selected, machine).await?;
            if !inventory.iter().any(|status| {
                target.binding.as_ref() == Some(&status.binding)
                    && status.observed
                    && status.desired_replicas == target.target_replicas
                    && status.ready_replicas >= target.target_replicas
            }) {
                return Err("protected original pool lacks exact positive Ready capacity".into());
            }
        }
    }
    Ok(())
}

pub(super) async fn read(
    database: &Database,
    deployment: &str,
    selected: &ReleaseTarget,
    machine: bool,
) -> Result<PoolTarget, String> {
    let target = database
        .collection_with_options::<PoolTarget>(
            "native_warm_pool_targets",
            mongodb::options::CollectionOptions::builder()
                .read_concern(mongodb::options::ReadConcern::majority())
                .build(),
        )
        .find_one(doc! {"_id": &selected.target_id, "plan.deployment": deployment})
        .await
        .map_err(|_| "original pool target is unavailable")?
        .ok_or("original pool target is absent")?;
    if target.document_schema_version != if machine { 4 } else { 3 }
        || target.machine != machine
        || !target_matches(selected, &target)
    {
        return Err("original pool target identity, revision, schema or readiness differs".into());
    }
    Ok(target)
}
