//! Operator control uses Agent Manager's normal database authority.
use layer2_core::sandbox::SandboxStoreError;

use super::ReleasedLane;
use crate::store::native_warm_pools::MongoNativeWarmPools;
use crate::store::released_runtimes::MongoReleasedRuntimes;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(crate) async fn set_following(
    lanes: &MongoReleasedRuntimes,
    targets: &MongoNativeWarmPools,
    follow: bool,
) -> Result<(), SandboxStoreError> {
    for lane in [ReleasedLane::Agent, ReleasedLane::Machine] {
        let Some(mut current) = lanes.load(lane).await? else {
            continue;
        };
        if !follow && !current.frozen {
            let mut session = lanes.session().await?;
            let next = current.freeze().map_err(|_| SandboxStoreError::Conflict)?;
            commit_lane(lanes, &current, &next, &mut session).await?;
            current = next;
        }
        if current.frozen && current.candidate.is_some() {
            let mut session = lanes.session().await?;
            match targets
                .clear_candidate(
                    lane == ReleasedLane::Machine,
                    current.candidate.as_ref(),
                    &mut session,
                )
                .await
            {
                Ok(()) => {}
                Err(layer1_public::rest::admin::warm_pools::WarmPoolError::Conflict) if !follow => {
                    // Freeze is already durable. Preserve the observation until
                    // its claimed provider effects have passed their bound.
                    continue;
                }
                Err(_) => return Err(unavailable()),
            }
            let next = current
                .clear_frozen_candidate()
                .map_err(|_| SandboxStoreError::Conflict)?;
            commit_lane(lanes, &current, &next, &mut session).await?;
            current = next;
        }
        if follow && current.frozen {
            let mut session = lanes.session().await?;
            let next = current
                .enable_following()
                .map_err(|_| SandboxStoreError::Conflict)?;
            commit_lane(lanes, &current, &next, &mut session).await?;
        }
    }
    Ok(())
}

async fn commit_lane(
    lanes: &MongoReleasedRuntimes,
    current: &layer2_core::released_runtime::ReleasedLaneRecord,
    next: &layer2_core::released_runtime::ReleasedLaneRecord,
    session: &mut mongodb::ClientSession,
) -> Result<(), SandboxStoreError> {
    lanes
        .replace(current, next.clone(), Some(&mut *session))
        .await?;
    match session.commit_guarded().await {
        Ok(()) => Ok(()),
        Err(error) if error.contains_label("UnknownTransactionCommitResult") => {
            if lanes.load(current.lane).await?.is_some_and(|observed| {
                observed.revision == next.revision
                    && observed.frozen == next.frozen
                    && observed.candidate == next.candidate
            }) {
                Ok(())
            } else {
                Err(unavailable())
            }
        }
        Err(_) => Err(unavailable()),
    }
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
                return Err(format!(
                    "{lane:?} freeze is durable, but candidate provider work has not settled"
                ));
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
