use super::*;
use std::{collections::VecDeque, time::Duration};

struct Observations {
    client: AgentSandboxClient,
    calls: Arc<Mutex<Vec<http::Uri>>>,
}

impl Observations {
    fn new(sandboxes: Vec<(u16, Value)>, pods: Vec<(u16, Value)>) -> Self {
        let sandboxes = Arc::new(Mutex::new(VecDeque::from(sandboxes)));
        let pods = Arc::new(Mutex::new(VecDeque::from(pods)));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let is_pods = request.uri().path().ends_with("/pods");
            assert!(
                is_pods
                    || request.uri().path().contains("/sandboxes")
                    || request.uri().path().contains("/persistentvolumeclaims/")
            );
            observed.lock().unwrap().push(request.uri().clone());
            let replies = if is_pods { &pods } else { &sandboxes };
            let (status, body) = replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected read");
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        Self {
            client: AgentSandboxClient::new(
                kube::Client::new(service, "cma-test"),
                super::super::tests::config(),
                "fixture",
            )
            .unwrap(),
            calls,
        }
    }

    fn order(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|uri| {
                if uri.path().ends_with("/pods") {
                    "pods"
                } else if uri.path().contains("/persistentvolumeclaims/") {
                    "pvc"
                } else {
                    "sandbox"
                }
            })
            .collect()
    }
}

#[tokio::test]
async fn readiness_retry_requires_fresh_generation_proof_and_preserves_original_pod_binding() {
    let (handle, object, pods) = pod_read_tests::ready();
    let mut stale = object.clone();
    stale["metadata"]["generation"] = json!(2);
    let reads = Observations::new(
        vec![(200, stale), (200, object)],
        vec![(200, pods.clone()), (200, pods)],
    );
    let binding = reads
        .client
        .wait_ready(&handle, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(binding.sandbox_uid, "sandbox-uid");
    assert_eq!(binding.pod_uid, "pod-uid");
    assert_eq!(reads.calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn readiness_retry_rejects_replacement_uid_instead_of_accepting_a_ready_pod() {
    let (handle, mut object, pods) = pod_read_tests::ready();
    let mut pending = object.clone();
    pending["status"]["conditions"][0]["status"] = json!("False");
    object["metadata"]["uid"] = json!("replacement");
    let reads = Observations::new(
        vec![(200, pending), (200, object)],
        vec![(200, pods.clone()), (200, pods)],
    );
    assert!(matches!(
        reads
            .client
            .wait_ready(&handle, Duration::from_secs(5))
            .await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(reads.calls.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn readiness_deadline_stops_pending_observation_without_a_second_read() {
    let (handle, mut object, pods) = pod_read_tests::ready();
    object["status"]["conditions"][0]["status"] = json!("False");
    let reads = Observations::new(vec![(200, object)], vec![(200, pods)]);
    assert!(matches!(
        reads.client.wait_ready(&handle, Duration::from_millis(20)).await,
        Err(AdapterError::Unavailable(detail)) if detail == "Sandbox readiness is still pending"
    ));
    assert_eq!(reads.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn suspension_retry_revalidates_spec_before_observing_original_pod_absence() {
    for changed in [false, true] {
        let (handle, mut object, mut pods) = pod_read_tests::ready();
        object["spec"]["operatingMode"] = json!("Suspended");
        let desired: kube::api::DynamicObject = serde_json::from_value(object.clone()).unwrap();
        let mut final_object = object.clone();
        if changed {
            final_object["spec"]["operatingMode"] = json!("Running");
        }
        // A terminating Pod is still a possible writer and cannot prove suspension.
        pods["items"][0]["metadata"]["deletionTimestamp"] = json!("2026-10-20T00:00:00Z");
        let reads = Observations::new(
            vec![(200, object), (200, final_object)],
            vec![(200, pods), (200, list("PodList"))],
        );
        let result = reads.client.wait_suspended(&handle, &desired).await;
        if changed {
            assert!(matches!(result, Err(AdapterError::Rejected(_))));
            assert_eq!(reads.order(), ["sandbox", "pods", "sandbox"]);
        } else {
            result.unwrap();
            assert_eq!(reads.order(), ["sandbox", "pods", "sandbox", "pods"]);
        }
    }
}

#[tokio::test]
async fn deletion_retry_requires_complete_fresh_absence_after_a_lingering_original_pod() {
    for partial in [false, true] {
        let (handle, _, pods) = pod_read_tests::ready();
        let mut absent_pods = list("PodList");
        let mut pod_reads = vec![(200, pods)];
        if partial {
            absent_pods["metadata"]["continue"] = json!("another-page");
            pod_reads.push((200, absent_pods.clone()));
        }
        pod_reads.push((200, absent_pods));
        let reads = Observations::new(
            vec![
                (404, missing()),
                (404, missing()),
                (200, list("SandboxList")),
                (200, list("SandboxList")),
            ],
            pod_reads,
        );
        let result = reads.client.delete(&handle).await;
        let mut expected = vec!["sandbox", "pvc", "sandbox", "pods", "sandbox", "pods"];
        if partial {
            assert!(matches!(result, Err(AdapterError::Unavailable(_))));
            expected.push("pods");
            assert!(reads.calls.lock().unwrap()[6]
                .query()
                .unwrap()
                .contains("continue=another-page"));
        } else {
            result.unwrap();
        }
        assert_eq!(reads.order(), expected);
    }
}
