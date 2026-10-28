use super::*;

#[path = "idle_bootstrap_tests.rs"]
mod idle_bootstrap;
#[path = "terminated_ephemeral_tests.rs"]
mod terminated_ephemeral;

async fn crashed() -> (Bound, HomeVolumeAttachment) {
    let bound = bound_with_mode(false, HomeVolumeMode::NativeVctFullHomeV2).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    let mut state = bound.fixture.state.lock().unwrap();
    state.rows.get_mut(MEMBER).unwrap()["status"]["conditions"][0]["status"] = json!("False");
    let template = state.rows[MEMBER]["spec"]["podTemplate"]["spec"].clone();
    let pod = state.rows.get_mut(POD).unwrap();
    pod["spec"] = template;
    pod["spec"]["volumes"] = json!([{"name":"home","persistentVolumeClaim":{"claimName":home.receipt.storage_reference}}]);
    pod["metadata"]["resourceVersion"] = json!("72");
    pod["status"]["conditions"][0]["status"] = json!("False");
    pod["status"]["initContainerStatuses"] = json!([{
        "name":"initialize-home","image":"runtime","imageID":"image-id","ready":true,"restartCount":0,
        "state":{"terminated":{"exitCode":0,"finishedAt":"2026-10-28T06:00:00Z"}}
    }]);
    pod["status"]["containerStatuses"][0] = json!({
        "name":"runtime","image":"runtime","imageID":"image-id","containerID":"containerd://original",
        "ready":false,"started":false,"restartCount":20,
        "state":{"waiting":{"reason":"CrashLoopBackOff"}},
        "lastState":{"terminated":{"exitCode":1,"reason":"Error","containerID":"containerd://original",
            "startedAt":"2026-10-28T06:00:00Z","finishedAt":"2026-10-28T06:00:01Z"}}
    });
    state.calls.clear();
    drop(state);
    (bound, home)
}

#[tokio::test]
async fn nonexecuting_proof_reads_original_graph_without_ready_exec_or_runtime_binding() {
    let (bound, home) = crashed().await;
    let before = bound.fixture.state.lock().unwrap().rows.clone();
    let binds = bound.transport.binds.lock().unwrap().clone();
    let proof = bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proof.runtime_restart_count, 20);
    assert_eq!(proof.runtime_container_id, "containerd://original");
    let encoded = serde_json::to_vec(&proof).unwrap();
    assert!(encoded.len() < 4096);
    let replay = serde_json::from_slice(&encoded).unwrap();
    bound
        .backend
        .verify_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &replay)
        .await
        .unwrap();
    let state = bound.fixture.state.lock().unwrap();
    assert_eq!(state.rows, before);
    assert!(state
        .calls
        .iter()
        .all(|(method, _)| method.starts_with("GET ")));
    for resource in [
        "/sandboxclaims/",
        "/sandboxes/",
        "/persistentvolumeclaims/",
        "/pods",
    ] {
        assert!(
            state
                .calls
                .iter()
                .filter(|(method, _)| method.contains(resource))
                .count()
                >= 2
        );
    }
    assert_eq!(*bound.transport.binds.lock().unwrap(), binds);
}

#[tokio::test]
async fn nonexecuting_proof_rejects_execution_uncertainty_and_foreign_authority() {
    for change in [
        "running",
        "ready",
        "init_running",
        "init_missing",
        "ephemeral",
        "success",
        "signal",
        "no_finished",
        "unknown_wait",
        "missing_container",
        "container_identity",
        "image",
        "home_mount",
        "foreign_pod",
        "foreign_volume",
        "foreign_claim",
        "member_uid",
        "request",
    ] {
        let (mut bound, home) = crashed().await;
        {
            let mut state = bound.fixture.state.lock().unwrap();
            match change {
                "foreign_volume" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("foreign")
                }
                "member_uid" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["uid"] = json!("foreign")
                }
                "foreign_claim" => {
                    let path = format!(
                        "{}/sandboxclaims/{}",
                        crate::agent_sandbox::pool::fixture::prefix(),
                        bound.claim.name
                    );
                    state.rows.get_mut(&path).unwrap()["metadata"]["uid"] = json!("foreign");
                }
                "request" => {
                    bound
                        .request
                        .env
                        .insert("UNAPPROVED".into(), "value".into());
                }
                _ => {
                    let pod = state.rows.get_mut(POD).unwrap();
                    match change {
                        "running" => {
                            pod["status"]["containerStatuses"][0]["state"] = json!({"running":{}})
                        }
                        "ready" => pod["status"]["conditions"][0]["status"] = json!("True"),
                        "init_running" => {
                            pod["status"]["initContainerStatuses"][0]["state"] =
                                json!({"running":{}})
                        }
                        "init_missing" => pod["status"]["initContainerStatuses"] = json!([]),
                        "ephemeral" => {
                            pod["status"]["ephemeralContainerStatuses"] = json!([{"name":"debug","image":"runtime","imageID":"id","ready":true,"restartCount":0,"state":{"running":{}}}])
                        }
                        "success" => {
                            pod["status"]["containerStatuses"][0]["lastState"]["terminated"]
                                ["exitCode"] = json!(0)
                        }
                        "signal" => {
                            pod["status"]["containerStatuses"][0]["lastState"]["terminated"]
                                ["signal"] = json!(9)
                        }
                        "no_finished" => {
                            pod["status"]["containerStatuses"][0]["lastState"]["terminated"]
                                .as_object_mut()
                                .unwrap()
                                .remove("finishedAt");
                        }
                        "unknown_wait" => {
                            pod["status"]["containerStatuses"][0]["state"]["waiting"]["reason"] =
                                json!("Unknown")
                        }
                        "missing_container" => pod["status"]["containerStatuses"] = json!([]),
                        "container_identity" => {
                            pod["status"]["containerStatuses"][0]["containerID"] =
                                json!("containerd://foreign")
                        }
                        "image" => pod["spec"]["containers"][0]["image"] = json!("foreign"),
                        "home_mount" => {
                            pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"] =
                                json!("foreign")
                        }
                        "foreign_pod" => {
                            pod["metadata"]["ownerReferences"][0]["uid"] = json!("foreign")
                        }
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
async fn nonexecuting_final_verification_rejects_restart_and_replacement() {
    for (pointer, value) in [
        ("/metadata/uid", json!("replacement")),
        ("/metadata/resourceVersion", json!("73")),
        ("/status/containerStatuses/0/restartCount", json!(21)),
        ("/status/containerStatuses/0/state", json!({"running":{}})),
        (
            "/status/containerStatuses/0/lastState/terminated/finishedAt",
            json!("2026-10-28T06:00:02Z"),
        ),
    ] {
        let (bound, home) = crashed().await;
        let proof = bound
            .backend
            .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
            .await
            .unwrap()
            .unwrap();
        *bound
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .get_mut(POD)
            .unwrap()
            .pointer_mut(pointer)
            .unwrap() = value;
        assert!(
            bound
                .backend
                .verify_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof)
                .await
                .is_err(),
            "{pointer}"
        );
        assert!(
            bound
                .backend
                .suspend_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof,)
                .await
                .is_err(),
            "{pointer}"
        );
        assert!(bound
            .fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(method, _)| method.starts_with("GET ")));
    }
}

#[tokio::test]
async fn nonexecuting_inspection_shares_bounded_capacity_and_never_queues() {
    let (bound, home) = crashed().await;
    let client = client(&bound.fixture, false);
    let clone = client.clone();
    let permits = client
        .runtime_requests
        .clone()
        .acquire_many_owned(32)
        .await
        .unwrap();
    assert!(matches!(
        clone
            .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
            .await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert!(bound.fixture.state.lock().unwrap().calls.is_empty());
    drop(permits);
    assert!(clone
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .is_some());
}

struct NoAgentRuntime;
impl crate::hosted::HostedAgentRuntime for NoAgentRuntime {
    fn home_plan(&self, _: &AgentHomeRequest) -> Result<AgentHomePlan> {
        panic!("no Agent home effects")
    }
    fn launch(&self, _: &AgentOpenRequest) -> Result<AgentLaunch> {
        panic!("no Agent launch")
    }
}

#[tokio::test]
async fn nonexecuting_hosted_observation_validates_request_and_original_home() {
    use crate::agent_sandbox::NativeHomeVolumes;
    use crate::hosted::{HostedAdapter, HostedDeployment};
    let (bound, home) = crashed().await;
    let spec = super::super::super::tests::spec();
    let backend = Arc::new(AgentSandboxBackend::new(
        client(&bound.fixture, false),
        bound.transport.clone(),
    ));
    let adapter = HostedAdapter::new(
        backend,
        Arc::new(NoAgentRuntime),
        HostedDeployment {
            image: spec.image.clone(),
            ..Default::default()
        },
    )
    .with_home_volumes(Arc::new(
        NativeHomeVolumes::new(
            bound.fixture.kubernetes.clone(),
            bound.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap(),
    ));
    let mut request = RuntimeAdoptionRequest::new(
        1,
        bound.runtime.clone(),
        SandboxSpec {
            sandbox_id: home.request.sandbox_id.clone(),
            runtime_image: Some(spec.image.uri),
            created_by: Some("cma-test".into()),
            home_volume: Some(Box::new(home)),
            capacity: SandboxCapacity {
                cpu_millis: spec.cpu_millis,
                memory_mib: spec.memory_mib,
            },
            ..Default::default()
        },
    );
    let proof = adapter
        .inspect_nonexecuting_runtime(&request)
        .await
        .unwrap()
        .unwrap();
    adapter
        .verify_nonexecuting_runtime(&request, &proof)
        .await
        .unwrap();
    request
        .spec
        .environment
        .insert("UNAPPROVED".into(), "value".into());
    assert!(adapter
        .verify_nonexecuting_runtime(&request, &proof)
        .await
        .is_err());
    request.spec.environment.clear();
    bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(VOLUME)
        .unwrap()["metadata"]["uid"] = json!("foreign");
    assert!(adapter
        .inspect_nonexecuting_runtime(&request)
        .await
        .is_err());
    assert!(bound
        .fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(method, _)| method.starts_with("GET ")));
}

#[tokio::test]
async fn nonexecuting_proof_accepts_current_error_termination_without_waiting_state() {
    let (bound, home) = crashed().await;
    {
        let mut state = bound.fixture.state.lock().unwrap();
        let pod = state.rows.get_mut(POD).unwrap();
        pod["status"]["phase"] = json!("Failed");
        let container = &mut pod["status"]["containerStatuses"][0];
        container["state"] = container["lastState"].clone();
        container["lastState"] = json!({});
    }
    assert!(bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn nonexecuting_proof_accepts_api_omitted_empty_fields_and_false_mount_defaults() {
    let (bound, home) = crashed().await;
    {
        let mut state = bound.fixture.state.lock().unwrap();
        let spec = &mut state.rows.get_mut(POD).unwrap()["spec"];
        for field in ["containers", "initContainers"] {
            for container in spec[field].as_array_mut().unwrap() {
                for empty in ["args", "env", "envFrom"] {
                    if container[empty].as_array().is_some_and(Vec::is_empty) {
                        container.as_object_mut().unwrap().remove(empty);
                    }
                }
                for mount in container["volumeMounts"].as_array_mut().unwrap() {
                    mount.as_object_mut().unwrap().remove("readOnly");
                }
            }
        }
    }
    assert!(bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn nonexecuting_suspension_uses_observed_cas_and_keeps_original_home() {
    let (bound, home) = crashed().await;
    let proof = bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    let before = bound.fixture.state.lock().unwrap().rows[VOLUME].clone();
    let controller = async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                {
                    let mut state = bound.fixture.state.lock().unwrap();
                    if state.rows[MEMBER]["spec"]["operatingMode"] == "Suspended" {
                        state.rows.remove(POD);
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    };
    let (result, ()) = tokio::join!(
        bound
            .backend
            .suspend_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof),
        controller,
    );
    result.unwrap();
    let state = bound.fixture.state.lock().unwrap();
    let changes: Vec<_> = state
        .calls
        .iter()
        .filter(|(method, _)| !method.starts_with("GET "))
        .collect();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].0.starts_with("PATCH "));
    assert_eq!(
        changes[0].1["metadata"]["resourceVersion"],
        proof.sandbox_resource_version
    );
    assert_eq!(changes[0].1["spec"], json!({"operatingMode":"Suspended"}));
    assert_eq!(state.rows[VOLUME], before);
    assert!(!state
        .calls
        .iter()
        .any(|(method, _)| method.contains("/exec")));
}
