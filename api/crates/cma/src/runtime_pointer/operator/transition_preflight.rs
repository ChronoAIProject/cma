use layer2_core::released_runtime::ReleasedLane;

use crate::store::released_runtimes::{MongoReleasedRuntimes, TransitionDestination};

const ENABLED: &str = "CHRONO_RUNTIME_RELEASE_TRANSITION_PREFLIGHT_ENABLED";

fn opted_in(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(format!("{ENABLED} must be true or false")),
    }
}

fn destination<'a>(
    lane: ReleasedLane,
    revision: &str,
    target_id: &'a str,
) -> Result<TransitionDestination<'a>, String> {
    let expected_revision = revision
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && i64::try_from(*value).is_ok())
        .ok_or_else(|| {
            format!("{lane:?} lane revision must be a positive signed 64-bit integer")
        })?;
    if target_id.is_empty()
        || target_id.len() > 128
        || !target_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(format!("{lane:?} target ID is invalid"));
    }
    Ok(TransitionDestination {
        lane,
        expected_revision,
        target_id,
    })
}

pub(super) async fn command(arguments: &[String]) -> Result<(), String> {
    let configured = match std::env::var(ENABLED) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{ENABLED} must be true or false"));
        }
    };
    if !opted_in(configured.as_deref())? {
        return Err("runtime release transition preflight is disabled".into());
    }
    let [agent_revision, agent_target, machine_revision, machine_target] = arguments else {
        return Err("usage: cma --runtime-pointer transition-preflight <agent-lane-revision> <agent-target-id> <machine-lane-revision> <machine-target-id>".into());
    };
    let agent = destination(ReleasedLane::Agent, agent_revision, agent_target)?;
    let machine = destination(ReleasedLane::Machine, machine_revision, machine_target)?;
    let config = match crate::config::StartupConfig::from_env()? {
        crate::config::StartupConfig::Execution(config) => config,
        _ => return Err("runtime release preflight requires execution configuration".into()),
    };
    let client = mongodb::Client::with_uri_str(&config.mongodb_uri)
        .await
        .map_err(|_| "connect to Agent Manager database")?;
    let database = client.database(&config.mongodb_database);
    let lanes = MongoReleasedRuntimes::open(&database, &config.deployment_id);
    let pair = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        lanes.inspect_transition_pair(agent, machine),
    )
    .await
    .map_err(|_| "retained runtime metadata read timed out")?
    .map_err(|error| match error {
        layer2_core::sandbox::SandboxStoreError::Conflict => {
            "retained runtime metadata did not match the proposed pair"
        }
        _ => "retained runtime metadata could not be verified",
    })?;
    println!(
        "{}",
        serde_json::json!({"check":"retained_metadata_only", "destinations":pair})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_transition_preflight_is_strictly_opt_in() {
        assert!(!opted_in(None).unwrap());
        assert!(!opted_in(Some("false")).unwrap());
        assert!(opted_in(Some("true")).unwrap());
        assert!(opted_in(Some("1")).is_err());
    }

    #[test]
    fn operator_transition_preflight_requires_exact_bounded_arguments() {
        let input = destination(ReleasedLane::Agent, "7", "wp_example").unwrap();
        assert_eq!(input.lane, ReleasedLane::Agent);
        assert_eq!(input.expected_revision, 7);
        assert_eq!(input.target_id, "wp_example");
        for (revision, target) in [
            ("0", "pool"),
            ("9223372036854775808", "pool"),
            ("4", "pool/name"),
        ] {
            assert!(destination(ReleasedLane::Agent, revision, target).is_err());
        }
    }
}
