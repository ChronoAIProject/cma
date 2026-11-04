use super::*;
use crate::agent_sandbox::pool::runtime_tests::{client, request};
use crate::{HomeVolumeAttachment, RuntimeHandle};
const POD: &str = "/api/v1/namespaces/cma-test/pods/cma-claim-cold";
async fn setup() -> (Fixture, HomeVolumeAttachment, RuntimeHandle) {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let handle = client(&fixture, false)
        .admit_pool_runtime(&claim, &home, &request(&home, 1))
        .await
        .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        let mut pod_spec = state.rows[MEMBER]["spec"]["podTemplate"]["spec"].clone();
        pod_spec["volumes"] =
            json!([{"name":"home","persistentVolumeClaim":{"claimName":"home-warm-member"}}]);
        state.rows.insert(POD.into(), json!({"apiVersion":"v1","kind":"Pod",
            "metadata":{"name":"cma-claim-cold","namespace":"cma-test","uid":"pod-uid",
                "labels":{"cma/control-plane":"cma-test","agents.x-k8s.io/sandbox-name-hash":crate::agent_sandbox::pods::name_hash("warm-member")},
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]},
            "spec":pod_spec,
            "status":{"phase":"Running","podIP":"10.0.0.2","conditions":[{"type":"Ready","status":"True"}],
                "containerStatuses":[{"name":"runtime","image":"runtime","imageID":"sha256:fixture","ready":true,"restartCount":0,"containerID":"containerd://first","state":{"running":{}}}]}}));
        state.calls.clear();
    }
    (fixture, home, handle)
}
fn readonly(fixture: &Fixture) {
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}
#[tokio::test]
async fn metrics_identity_accepts_cold_names_and_counts_multi_mount_home_once() {
    let (fixture, home, handle) = setup().await;
    let identity = client(&fixture, false)
        .metric_identity(&home, Some((&handle, 1)))
        .await
        .unwrap();
    assert_eq!(identity.pvc_uid, "pvc-uid");
    let runtime = identity.runtime.unwrap();
    assert_eq!(runtime.pod.pod_name, "cma-claim-cold");
    assert_eq!(
        runtime.pod.runtime_container_id.as_deref(),
        Some("containerd://first")
    );
    assert_eq!(runtime.cpu_limit_cores, Some(0.75));
    readonly(&fixture);
}
#[tokio::test]
async fn metrics_identity_keeps_verified_home_when_runtime_is_absent_or_replaced() {
    for change in ["deleted", "owner", "container", "generation"] {
        let (fixture, home, handle) = setup().await;
        {
            let mut state = fixture.state.lock().unwrap();
            match change {
                "deleted" => {
                    state.rows.remove(POD);
                }
                "owner" => {
                    state.rows.get_mut(POD).unwrap()["metadata"]["ownerReferences"][0]["uid"] =
                        json!("another")
                }
                "container" => {
                    state.rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0]
                        ["containerID"] = json!("")
                }
                "generation" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["annotations"]
                        ["cma/runtime-generation"] = json!("2")
                }
                _ => unreachable!(),
            }
        }
        let identity = client(&fixture, false)
            .metric_identity(&home, Some((&handle, 1)))
            .await
            .unwrap();
        assert_eq!(identity.pvc_uid, "pvc-uid");
        assert!(identity.runtime.is_none(), "{change}");
        readonly(&fixture);
    }
}
#[tokio::test]
async fn metrics_identity_refuses_reused_pvc_and_stale_persisted_receipts() {
    for change in ["pvc", "receipt", "owner", "deployment"] {
        let (fixture, mut home, handle) = setup().await;
        match change {
            "pvc" => {
                fixture.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
                    json!("replacement")
            }
            "receipt" => home.receipt.native_authority.as_mut().unwrap().pvc_uid = "stale".into(),
            "owner" => {
                fixture.state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["metadata"]
                    ["ownerReferences"][0]["uid"] = json!("unassigned-pool")
            }
            "deployment" => {
                home.receipt
                    .native_authority
                    .as_mut()
                    .unwrap()
                    .claim
                    .request
                    .pool
                    .config_sha256 = "0".repeat(64)
            }
            _ => unreachable!(),
        }
        assert!(
            client(&fixture, false)
                .metric_identity(&home, Some((&handle, 1)))
                .await
                .is_err(),
            "{change}"
        );
        readonly(&fixture);
    }
}
#[tokio::test]
async fn metrics_identity_exposes_new_pod_and_container_incarnation_and_paused_home() {
    let (fixture, home, handle) = setup().await;
    let first = client(&fixture, false)
        .metric_identity(&home, Some((&handle, 1)))
        .await
        .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        let pod = state.rows.get_mut(POD).unwrap();
        pod["metadata"]["uid"] = json!("new-pod-uid");
        pod["status"]["containerStatuses"][0]["containerID"] = json!("containerd://second");
        pod["status"]["containerStatuses"][0]["restartCount"] = json!(1);
    }
    let second = client(&fixture, false)
        .metric_identity(&home, Some((&handle, 1)))
        .await
        .unwrap();
    assert_ne!(first.runtime.unwrap().pod, second.runtime.unwrap().pod);
    let paused = client(&fixture, false)
        .metric_identity(&home, None)
        .await
        .unwrap();
    assert_eq!(paused.pvc_uid, first.pvc_uid);
    assert!(paused.runtime.is_none());
    readonly(&fixture);
}
