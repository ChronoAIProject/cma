use super::*;

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
    let parsed = lines(DEPLOYMENT);
    let pod = parsed
        .iter()
        .position(|line| line.indent == 4 && line.text == "spec:")
        .unwrap();
    let grace = child(&parsed, pod, "terminationGracePeriodSeconds").unwrap();
    assert_eq!(
        value(&parsed[grace], "terminationGracePeriodSeconds"),
        Some("735")
    );
    let owner = container(&parsed, "cmaeg").unwrap();
    let lifecycle = child(&parsed, owner, "lifecycle").unwrap();
    let pre_stop = child(&parsed, lifecycle, "preStop").unwrap();
    let sleep = child(&parsed, pre_stop, "sleep").unwrap();
    let seconds = child(&parsed, sleep, "seconds").unwrap();
    assert_eq!(value(&parsed[seconds], "seconds"), Some("30"));
}
