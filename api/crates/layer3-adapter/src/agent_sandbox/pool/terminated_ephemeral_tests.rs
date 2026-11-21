use super::*;

fn completed_diagnostic(bound: &Bound) {
    let mut state = bound.fixture.state.lock().unwrap();
    let pod = state.rows.get_mut(POD).unwrap();
    pod["spec"]["ephemeralContainers"] = json!([{
        "name":"diagnostic","image":"runtime",
        "volumeMounts":[{"name":"home","mountPath":"/evidence","readOnly":true}]
    }]);
    pod["status"]["ephemeralContainerStatuses"] = json!([{
        "name":"diagnostic","image":"sha256:resolved-image","imageID":"runtime",
        "containerID":"containerd://diagnostic","ready":false,"restartCount":0,
        "lastState":{},
        "state":{"terminated":{"exitCode":0,"reason":"Completed",
            "containerID":"containerd://diagnostic",
            "startedAt":"2026-10-27T06:00:00Z","finishedAt":"2026-10-27T06:00:01Z"}}
    }]);
}

#[tokio::test]
async fn completed_ephemeral_diagnostic_preserves_readonly_nonexecuting_proof() {
    let (bound, home) = crashed().await;
    completed_diagnostic(&bound);
    let before = bound.fixture.state.lock().unwrap().rows.clone();
    let binds = bound.transport.binds.lock().unwrap().clone();
    let proof = bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .expect("completed ephemeral container cannot restart");
    bound
        .backend
        .verify_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof)
        .await
        .unwrap();
    let state = bound.fixture.state.lock().unwrap();
    assert_eq!(state.rows, before);
    assert!(state
        .calls
        .iter()
        .all(|(method, _)| method.starts_with("GET ")));
    assert_eq!(*bound.transport.binds.lock().unwrap(), binds);
}

#[tokio::test]
async fn ephemeral_execution_or_incomplete_identity_never_proves_nonexecution() {
    for change in [
        "running",
        "waiting",
        "missing_state",
        "missing_termination",
        "missing_spec",
        "missing_status",
        "extra_status",
        "duplicate_status",
        "duplicate_spec",
        "foreign_name",
        "ready",
        "started",
        "restarted",
        "container_identity",
        "missing_container_identity",
        "missing_started",
        "missing_finished",
        "reversed_times",
        "previous_execution",
    ] {
        let (bound, home) = crashed().await;
        completed_diagnostic(&bound);
        {
            let mut state = bound.fixture.state.lock().unwrap();
            let pod = state.rows.get_mut(POD).unwrap();
            match change {
                "missing_spec" => pod["spec"]["ephemeralContainers"] = json!([]),
                "missing_status" => pod["status"]["ephemeralContainerStatuses"] = json!([]),
                "extra_status" | "duplicate_status" => {
                    let mut extra = pod["status"]["ephemeralContainerStatuses"][0].clone();
                    if change == "extra_status" {
                        extra["name"] = json!("foreign");
                    }
                    pod["status"]["ephemeralContainerStatuses"]
                        .as_array_mut()
                        .unwrap()
                        .push(extra);
                }
                "duplicate_spec" => {
                    let duplicate = pod["spec"]["ephemeralContainers"][0].clone();
                    pod["spec"]["ephemeralContainers"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                    let mut extra = pod["status"]["ephemeralContainerStatuses"][0].clone();
                    extra["name"] = json!("foreign");
                    pod["status"]["ephemeralContainerStatuses"]
                        .as_array_mut()
                        .unwrap()
                        .push(extra);
                }
                _ => {
                    let status = &mut pod["status"]["ephemeralContainerStatuses"][0];
                    match change {
                        "running" => status["state"]["running"] = json!({}),
                        "waiting" => {
                            status["state"]["waiting"] = json!({"reason":"ContainerCreating"})
                        }
                        "missing_state" => status["state"] = json!(null),
                        "missing_termination" => status["state"] = json!({}),
                        "foreign_name" => status["name"] = json!("foreign"),
                        "ready" => status["ready"] = json!(true),
                        "started" => status["started"] = json!(true),
                        "restarted" => status["restartCount"] = json!(1),
                        "container_identity" => {
                            status["containerID"] = json!("containerd://foreign")
                        }
                        "missing_container_identity" => {
                            status["state"]["terminated"]["containerID"] = json!(null)
                        }
                        "missing_started" => {
                            status["state"]["terminated"]["startedAt"] = json!(null)
                        }
                        "missing_finished" => {
                            status["state"]["terminated"]["finishedAt"] = json!(null)
                        }
                        "reversed_times" => {
                            status["state"]["terminated"]["finishedAt"] =
                                json!("2026-10-26T06:00:00Z")
                        }
                        "previous_execution" => status["lastState"] = json!({"running":{}}),
                        _ => unreachable!(),
                    }
                }
            }
        }
        assert!(
            !matches!(
                bound
                    .backend
                    .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
                    .await,
                Ok(Some(_))
            ),
            "{change}"
        );
        assert!(
            bound
                .fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(method, _)| method.starts_with("GET ")),
            "{change}"
        );
    }
}

#[tokio::test]
async fn changed_completed_ephemeral_evidence_fences_verification_and_suspension() {
    for change in [
        "container_identity",
        "finished_at",
        "spec_image",
        "new_ephemeral",
    ] {
        let (bound, home) = crashed().await;
        completed_diagnostic(&bound);
        let proof = bound
            .backend
            .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
            .await
            .unwrap()
            .unwrap();
        {
            let mut state = bound.fixture.state.lock().unwrap();
            let pod = state.rows.get_mut(POD).unwrap();
            match change {
                "container_identity" => {
                    let status = &mut pod["status"]["ephemeralContainerStatuses"][0];
                    status["containerID"] = json!("containerd://replacement");
                    status["state"]["terminated"]["containerID"] =
                        json!("containerd://replacement");
                }
                "finished_at" => {
                    pod["status"]["ephemeralContainerStatuses"][0]["state"]["terminated"]
                        ["finishedAt"] = json!("2026-10-27T06:00:02Z")
                }
                "spec_image" => pod["spec"]["ephemeralContainers"][0]["image"] = json!("foreign"),
                "new_ephemeral" => {
                    let mut extra = pod["spec"]["ephemeralContainers"][0].clone();
                    extra["name"] = json!("later-diagnostic");
                    pod["spec"]["ephemeralContainers"]
                        .as_array_mut()
                        .unwrap()
                        .push(extra);
                    let mut extra = pod["status"]["ephemeralContainerStatuses"][0].clone();
                    extra["name"] = json!("later-diagnostic");
                    pod["status"]["ephemeralContainerStatuses"]
                        .as_array_mut()
                        .unwrap()
                        .push(extra);
                }
                _ => unreachable!(),
            }
        }
        // Keep the resourceVersion unchanged to exercise the full spec/status
        // digest, in addition to the existing UID/resourceVersion race tests.
        assert!(
            bound
                .backend
                .verify_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            bound
                .backend
                .suspend_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            bound
                .fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(method, _)| method.starts_with("GET ")),
            "{change}"
        );
    }
}
