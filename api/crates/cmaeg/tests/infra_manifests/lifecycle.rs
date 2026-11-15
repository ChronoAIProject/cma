use super::*;

const ROUTING_PROPAGATION_SECONDS: u64 = 30;
const PROCESS_TEARDOWN_SECONDS: u64 = 45;

fn positive_seconds(value: Option<&str>, name: &str) -> Result<u64, String> {
    value
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| format!("{name} must be a positive number of seconds"))
}

/// Configuration budgets include native preStop, connection drain and teardown;
/// they do not establish a measured process or client lifetime.
fn rollout_rule(configmap: &str, deployment: &str) -> Result<(), String> {
    let drain = positive_seconds(
        configmap_value(configmap, "CMAEG_DRAIN_SECONDS"),
        "CMAEG_DRAIN_SECONDS",
    )?;
    let parsed = lines(deployment);
    let pod = parsed
        .iter()
        .position(|line| line.indent == 4 && line.text == "spec:")
        .ok_or("the deployment has no pod spec")?;
    let grace = positive_seconds(
        child(&parsed, pod, "terminationGracePeriodSeconds")
            .and_then(|line| value(&parsed[line], "terminationGracePeriodSeconds")),
        "terminationGracePeriodSeconds",
    )?;
    let propagation = native_sleep_seconds(deployment)?;
    let required = propagation
        .checked_add(drain)
        .and_then(|seconds| seconds.checked_add(PROCESS_TEARDOWN_SECONDS))
        .ok_or("the termination budget overflows")?;
    if grace < required {
        return Err(format!(
            "termination grace {grace}s must cover propagation, drain and teardown ({required}s)"
        ));
    }
    Ok(())
}

pub(super) fn native_sleep_seconds(deployment: &str) -> Result<u64, String> {
    let parsed = lines(deployment);
    let owner = container(&parsed, "cmaeg")?;
    let pre_stop = child(&parsed, owner, "lifecycle")
        .and_then(|lifecycle| child(&parsed, lifecycle, "preStop"))
        .ok_or("cmaeg requires a native sleep preStop hook")?;
    let mut actions = parsed[pre_stop + 1..]
        .iter()
        .take_while(|line| line.indent > parsed[pre_stop].indent)
        .filter(|line| line.indent == parsed[pre_stop].indent + 2);
    if actions.next().is_none_or(|line| line.text != "sleep:") || actions.next().is_some() {
        return Err("cmaeg preStop must be a native sleep action, not exec or httpGet".into());
    }
    let propagation = positive_seconds(
        child(&parsed, pre_stop, "sleep")
            .and_then(|sleep| child(&parsed, sleep, "seconds"))
            .and_then(|seconds| value(&parsed[seconds], "seconds")),
        "preStop.sleep.seconds",
    )?;
    if propagation != ROUTING_PROPAGATION_SECONDS {
        return Err("cmaeg requires 30 seconds of routing propagation".into());
    }
    Ok(propagation)
}

fn with_grace(deployment: &str, seconds: &str) -> String {
    let parsed = lines(deployment);
    let line = parsed
        .iter()
        .find(|line| value(line, "terminationGracePeriodSeconds").is_some())
        .unwrap();
    deployment.replacen(
        line.text,
        &format!("terminationGracePeriodSeconds: {seconds}"),
        1,
    )
}

fn with_exec_pre_stop(deployment: &str) -> String {
    let edited = deployment.replace(
        "sleep:\n                seconds: 30",
        "exec:\n                command: [\"/bin/sleep\", \"30\"]",
    );
    assert_ne!(edited, deployment, "the shipped preStop hook changed shape");
    edited
}

#[test]
fn the_ffmpeg_dir_requires_a_sleep_pre_stop_and_2gi_of_guaranteed_memory() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_MEDIA_FFMPEG_DIR"),
        Some(FFMPEG_DIR)
    );
    assert_eq!(transformer_rule(CONFIGMAP, DEPLOYMENT), Ok(()));
    let ready = with_memory(DEPLOYMENT, "2Gi", "2Gi");
    let exec = with_exec_pre_stop(&ready);
    assert!(transformer_rule(CONFIGMAP, &exec)
        .unwrap_err()
        .contains("sleep action, not exec"));
    let disabled = set_config(CONFIGMAP, "CMAEG_MEDIA_FFMPEG_DIR", "");
    assert_eq!(transformer_rule(&disabled, &exec), Ok(()));
    let small = with_memory(DEPLOYMENT, "1536Mi", "1536Mi");
    assert!(transformer_rule(CONFIGMAP, &small)
        .unwrap_err()
        .contains("at least 2Gi"));
    let burstable = with_memory(DEPLOYMENT, "1536Mi", "2Gi");
    assert!(transformer_rule(CONFIGMAP, &burstable)
        .unwrap_err()
        .contains("to equal its limit"));
    assert_eq!(media_memory_rule(&enabled(CONFIGMAP), &ready), Ok(()));
}

#[test]
fn rollout_keeps_routing_propagation_and_connection_drain_margins() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_DRAIN_SECONDS"),
        Some("660")
    );
    assert_eq!(rollout_rule(CONFIGMAP, DEPLOYMENT), Ok(()));
    assert_eq!(
        rollout_rule(CONFIGMAP, &with_grace(DEPLOYMENT, "735")),
        Ok(())
    );
    assert!(rollout_rule(CONFIGMAP, &with_grace(DEPLOYMENT, "734"))
        .unwrap_err()
        .contains("must cover"));
    let longer_drain = set_config(CONFIGMAP, "CMAEG_DRAIN_SECONDS", "686");
    assert!(rollout_rule(&longer_drain, &with_grace(DEPLOYMENT, "760"))
        .unwrap_err()
        .contains("must cover"));
}

#[test]
fn rollout_rejects_missing_malformed_and_overflowing_budgets() {
    for invalid in ["", "0", "-1", "not-seconds", "18446744073709551616"] {
        let configmap = set_config(CONFIGMAP, "CMAEG_DRAIN_SECONDS", invalid);
        assert!(rollout_rule(&configmap, DEPLOYMENT).is_err(), "{invalid}");
        assert!(rollout_rule(CONFIGMAP, &with_grace(DEPLOYMENT, invalid)).is_err());
    }
    for seconds in [u64::MAX, u64::MAX - ROUTING_PROPAGATION_SECONDS] {
        let overflow = set_config(CONFIGMAP, "CMAEG_DRAIN_SECONDS", &seconds.to_string());
        assert!(rollout_rule(&overflow, DEPLOYMENT)
            .unwrap_err()
            .contains("overflows"));
    }
    let missing_drain = CONFIGMAP.replace("CMAEG_DRAIN_SECONDS:", "other:");
    assert!(rollout_rule(&missing_drain, DEPLOYMENT).is_err());
    let missing_grace = DEPLOYMENT.replace("terminationGracePeriodSeconds:", "other:");
    assert!(rollout_rule(CONFIGMAP, &missing_grace).is_err());
    let missing_pod = DEPLOYMENT.replace("    spec:\n", "    other:\n");
    assert!(rollout_rule(CONFIGMAP, &missing_pod).is_err());
}

#[test]
fn rollout_rejects_missing_extra_or_malformed_prestop_actions() {
    let exec = with_exec_pre_stop(DEPLOYMENT);
    let mixed = DEPLOYMENT.replace(
        "sleep:\n                seconds: 30",
        "sleep:\n                seconds: 30\n              exec:\n                command: [\"/bin/sleep\", \"30\"]",
    );
    for deployment in [
        exec,
        mixed,
        DEPLOYMENT.replace("lifecycle:", "other:"),
        DEPLOYMENT.replace("preStop:", "other:"),
        DEPLOYMENT.replace("sleep:", "other:"),
        DEPLOYMENT.replace("seconds: 30", "other: 30"),
        DEPLOYMENT.replace("sleep:", "httpGet:"),
        DEPLOYMENT.replace("- name: cmaeg", "- name: other"),
    ] {
        assert!(rollout_rule(CONFIGMAP, &deployment).is_err());
        assert!(transformer_rule(CONFIGMAP, &deployment).is_err());
    }
    for invalid in ["0", "29", "31", "-1", "bad", "18446744073709551616"] {
        let deployment = DEPLOYMENT.replace("seconds: 30", &format!("seconds: {invalid}"));
        assert!(rollout_rule(CONFIGMAP, &deployment).is_err(), "{invalid}");
        assert!(
            transformer_rule(CONFIGMAP, &deployment).is_err(),
            "{invalid}"
        );
    }
}
