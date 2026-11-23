//! Operator control uses Agent Manager's normal database authority.
use layer2_core::sandbox::SandboxStoreError;

use super::ReleasedLane;
use crate::store::native_warm_pools::MongoNativeWarmPools;
use crate::store::released_runtimes::MongoReleasedRuntimes;

pub(crate) async fn set_following(
    lanes: &MongoReleasedRuntimes,
    targets: &MongoNativeWarmPools,
    follow: bool,
) -> Result<(), SandboxStoreError> {
    for lane in [ReleasedLane::Agent, ReleasedLane::Machine] {
        let Some(current) = lanes.load(lane).await? else {
            continue;
        };
        if current.frozen != follow {
            continue;
        }
        let mut session = lanes.session().await?;
        let mut next = if follow {
            current.enable_following()
        } else {
            current.freeze()
        }
        .map_err(|_| SandboxStoreError::Conflict)?;
        if !follow {
            targets
                .clear_candidate(
                    lane == ReleasedLane::Machine,
                    current.candidate.as_ref(),
                    &mut session,
                )
                .await
                .map_err(|_| unavailable())?;
            next.candidate = None;
        }
        lanes
            .replace(&current, next.clone(), Some(&mut session))
            .await?;
        match session.commit_transaction().await {
            Ok(()) => {}
            Err(error) if error.contains_label("UnknownTransactionCommitResult") => {
                if lanes.load(lane).await?.is_none_or(|observed| {
                    observed.revision != next.revision
                        || observed.frozen != next.frozen
                        || observed.candidate != next.candidate
                }) {
                    return Err(unavailable());
                }
            }
            Err(_) => return Err(unavailable()),
        }
        tracing::info!(?lane, follow, "runtime pointer following changed");
    }
    Ok(())
}

pub(crate) async fn command(arguments: &[String]) -> Result<(), String> {
    let command = match arguments {
        [_, command] if matches!(command.as_str(), "freeze" | "enable" | "status") => {
            command.as_str()
        }
        _ => return Err("usage: cma --runtime-pointer <freeze|enable|status>".into()),
    };
    let config = match crate::config::StartupConfig::from_env()? {
        crate::config::StartupConfig::Execution(config) => config,
        _ => return Err("runtime pointer control requires execution configuration".into()),
    };
    if command == "enable"
        && !config
            .runtime
            .runtime_pointer
            .as_ref()
            .is_some_and(super::RuntimePointerConfig::can_follow)
    {
        return Err("runtime pointer following is disabled in this process configuration".into());
    }
    let client = mongodb::Client::with_uri_str(&config.mongodb_uri)
        .await
        .map_err(|_| "connect to Agent Manager database")?;
    let database = client.database(&config.mongodb_database);
    let lanes = MongoReleasedRuntimes::open(&database, &config.deployment_id);
    if lanes
        .load(ReleasedLane::Agent)
        .await
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err("the Agent Manager runtime lane has not bootstrapped".into());
    }
    if command != "status" {
        let targets = MongoNativeWarmPools::open(&database, &config.deployment_id)
            .await
            .map_err(|_| "open Agent Manager native pool store")?;
        set_following(&lanes, &targets, command == "enable")
            .await
            .map_err(|error| error.to_string())?;
    }
    for lane in [ReleasedLane::Agent, ReleasedLane::Machine] {
        if let Some(record) = lanes.load(lane).await.map_err(|error| error.to_string())? {
            if (command == "freeze" && (!record.frozen || record.candidate.is_some()))
                || (command == "enable" && record.frozen)
            {
                return Err("could not confirm the runtime pointer action".into());
            }
            println!(
                "{}",
                serde_json::to_string(&record).map_err(|error| error.to_string())?
            );
        }
    }
    Ok(())
}

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("runtime pointer operator action could not be confirmed".into())
}
