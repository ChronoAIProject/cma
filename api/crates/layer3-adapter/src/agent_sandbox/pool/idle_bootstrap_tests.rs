use super::*;

async fn idle() -> (Bound, HomeVolumeAttachment) {
    let image = "registry.example/runtime@sha256:3c5bccfabe8d20bd1fb5900cbfb2958878a316d75cfd1d186bcbfcf91c6fbf28";
    let bound = bound_with_image(false, HomeVolumeMode::NativeVctFullHomeV2, Some(image)).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    {
        let mut state = bound.fixture.state.lock().unwrap();
        let template = state.rows[MEMBER]["spec"]["podTemplate"]["spec"].clone();
        let pod = state.rows.get_mut(POD).unwrap();
        pod["spec"] = template;
        pod["spec"]["volumes"] = json!([{"name":"home","persistentVolumeClaim":{"claimName":home.receipt.storage_reference}}]);
        pod["metadata"]["resourceVersion"] = json!("72");
        pod["status"]["initContainerStatuses"] = json!([{
            "name":"initialize-home","image":"runtime","imageID":"image-id","ready":true,"restartCount":0,
            "state":{"terminated":{"exitCode":0,"finishedAt":"2026-10-28T06:00:00Z"}}
        }]);
        pod["status"]["containerStatuses"][0] = json!({
            "name":"runtime","image":"runtime","imageID":"image-id","containerID":"containerd://original",
            "ready":true,"started":true,"restartCount":20,
            "state":{"running":{"startedAt":"2026-10-28T06:00:02Z"}},
            "lastState":{"terminated":{"exitCode":2,"reason":"Error","containerID":"containerd://previous",
                "startedAt":"2026-10-28T06:00:00Z","finishedAt":"2026-10-28T06:00:01Z"}}
        });
        state.inspection_output = Some(json!({"status":"idle","proof":{
            "version":1,"pid_start_ticks":1234,
            "binary_sha256":"5a39d8c147fb9061e85831aa5fc7f42c405374719c4bc6d040e94dd2f997939b",
            "process_sha256":"b".repeat(64)
        }}));
        state.calls.clear();
    }
    (bound, home)
}

#[tokio::test]
async fn idle_proof_requires_exact_graph_then_inspects_without_assignment_or_write() {
    let (bound, home) = idle().await;
    let before = bound.fixture.state.lock().unwrap().rows.clone();
    let count = bound.fixture.state.lock().unwrap().assignments.len();
    let proof = bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proof.version, 2);
    assert_eq!(proof.idle_bootstrap.as_ref().unwrap().pid_start_ticks, 1234);
    bound
        .backend
        .verify_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof)
        .await
        .unwrap();
    let state = bound.fixture.state.lock().unwrap();
    assert_eq!(state.rows, before);
    assert_eq!(state.assignments.len(), count);
    assert_eq!(state.inspections, 2);
    assert!(state
        .calls
        .iter()
        .all(|(method, _)| method.starts_with("GET ")));
}

#[tokio::test]
async fn idle_refuses_process_change_replacement_assignment_and_unproved_binary() {
    for case in ["busy", "binary", "unknown", "restart", "pod", "request"] {
        let (bound, home) = idle().await;
        let proof = bound
            .backend
            .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
            .await
            .unwrap()
            .unwrap();
        {
            let mut state = bound.fixture.state.lock().unwrap();
            match case {
                "busy" => state.inspection_output = Some(json!({"status":"refused"})),
                "binary" => {
                    state.inspection_output.as_mut().unwrap()["proof"]["binary_sha256"] =
                        json!("c".repeat(64))
                }
                "unknown" => {
                    state.inspection_output.as_mut().unwrap()["proof"]["unproved"] = json!(true)
                }
                "restart" => state.exec_mutations.push((
                    POD.into(),
                    "/status/containerStatuses/0/restartCount".into(),
                    json!(21),
                )),
                "pod" => state.exec_mutations.push((
                    POD.into(),
                    "/metadata/uid".into(),
                    json!("replacement"),
                )),
                "request" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["annotations"]
                        ["cma/runtime-generation"] = json!("99")
                }
                _ => unreachable!(),
            }
        }
        assert!(
            bound
                .backend
                .verify_nonexecuting_runtime(&bound.runtime, &bound.request, &home, &proof)
                .await
                .is_err(),
            "{case}"
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
