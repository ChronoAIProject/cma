use super::{
    fixture::{Fixture, State},
    home_tests::{adopted, assigned, MEMBER, VOLUME},
    runtime_tests::{client, request},
};
use crate::{
    hosted::{AgentSandboxBackend, HostedAgentOpenSettings, HostedBackend, HostedControlTransport},
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    *,
};
use std::sync::{Arc, Mutex};

#[path = "backend_assignment_cache_tests.rs"]
mod assignment_cache;

struct Transport {
    state: Arc<Mutex<State>>,
    binds: Mutex<Vec<RuntimeHandle>>,
}

#[async_trait::async_trait]
impl HostedControlTransport for Transport {
    async fn bind_runtime(&self, handle: &RuntimeHandle, sandbox: &str) -> Result<()> {
        assert_eq!(sandbox, "sbx_one");
        assert!(
            !self.state.lock().unwrap().assignments.is_empty(),
            "assignment precedes transport binding"
        );
        self.binds.lock().unwrap().push(handle.clone());
        Ok(())
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("no control before assignment");
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("no Agent admission in provider test");
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn production_backend_assigns_original_warm_pod_before_binding_and_replays_after_restart() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    let request = request(&home, 1);
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.insert("/api/v1/namespaces/cma-test/pods/warm-member".into(), serde_json::json!({"apiVersion":"v1","kind":"Pod",
            "metadata":{"name":"warm-member","namespace":"cma-test","uid":"original-pod",
                "labels":{"cma/control-plane":"cma-test",super::super::pods::TRACKING_LABEL:super::super::pods::name_hash("warm-member")},
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]},
            "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}]}}));
        state.fail_exec = true;
    }
    let transport = Arc::new(Transport {
        state: fixture.state.clone(),
        binds: Mutex::new(vec![]),
    });
    let backend = || AgentSandboxBackend::new(client(&fixture, true), transport.clone());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        backend().provision(request.clone()),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(transport.binds.lock().unwrap().is_empty());
    fixture.state.lock().unwrap().fail_exec = false;
    let handle = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        backend().provision(request.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(handle.as_str(), "agent-sandbox:warm-member:member-uid");
    assert_eq!(
        backend()
            .adopt(&handle, &request)
            .await
            .unwrap()
            .unwrap()
            .handle,
        handle
    );
    assert_eq!(transport.binds.lock().unwrap().len(), 2);
    let pod_path = "/api/v1/namespaces/cma-test/pods/warm-member";
    let mut restarted_pod = fixture.state.lock().unwrap().rows.remove(pod_path).unwrap();
    backend().suspend(&handle, &request).await.unwrap();
    let next = super::runtime_tests::request(&home, 2);
    // Model a controller-observed restart before replaying the backend's lost
    // resume acknowledgement. The new process still starts in assignment mode.
    client(&fixture, true)
        .resume(&handle, &request, &next)
        .await
        .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        restarted_pod["metadata"]["uid"] = serde_json::json!("restarted-pod");
        state.rows.insert(pod_path.into(), restarted_pod);
        let member = state.rows.get_mut(MEMBER).unwrap();
        member["status"]["conditions"][0]["observedGeneration"] =
            member["metadata"]["generation"].clone();
    }
    backend().resume(&handle, &request, &next).await.unwrap();
    assert_eq!(transport.binds.lock().unwrap().len(), 3);
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.assignments.len(), 4);
    assert!(state.assignments[..3]
        .windows(2)
        .all(|pair| pair[0] == pair[1]));
    assert_eq!(state.assignments[0].pod_uid, "original-pod");
    let mut expected = request.env.clone();
    expected.insert(
        "CHRONO_RUNTIME_CONTROL_TOKEN".into(),
        client(&fixture, true).control_token("warm-member"),
    );
    assert_eq!(state.assignments[0].environment, expected);
    assert_eq!(state.assignments[3].pod_uid, "restarted-pod");
    assert_eq!(
        state.assignments[3].environment["CHRONO_RUNTIME_GENERATION"],
        "2"
    );
    assert_eq!(
        state.assignments[3].environment["CHRONO_RUNTIME_HOME_IDENTITY"],
        home.identity().unwrap()
    );
    assert_eq!(state.rows[VOLUME]["metadata"]["uid"], "pvc-uid");
    assert_eq!(
        state.rows[MEMBER]["spec"]["podTemplate"]["spec"]["containers"][0]["command"][1],
        "--wait-for-assignment"
    );
}
