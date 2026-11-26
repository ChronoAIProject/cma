use super::{
    fixture::Fixture,
    home_tests::{assigned_spec, MEMBER},
    *,
};
use crate::{AdapterError, HomeVolumeMode};
use serde_json::json;

const POD: &str = "/api/v1/namespaces/cma-test/pods/warm-pod";

mod authority_tests;

/// A wall-clock deadline lands either between polls, after a validated pending
/// read (Pending), or during an outstanding fixture read (TimedOut); a loaded
/// runner makes the second likelier. `agent_sandbox/claim_guard/tests.rs`
/// proves that exact classification with a paused clock. Fixture tests assert
/// a typed not-ready refusal and return the metric label it must record.
fn not_ready_outcome(error: &AdapterError) -> &'static str {
    match error {
        AdapterError::NativeClaimNotReady(super::super::NativeClaimGuardReason::Pending) => {
            "pending"
        }
        AdapterError::NativeClaimNotReady(super::super::NativeClaimGuardReason::TimedOut) => {
            "timeout"
        }
        other => panic!("expected a typed not-ready refusal, got {other:?}"),
    }
}
mod provenance_tests;

async fn fixture(ready: bool) -> (Fixture, NativePoolClaim) {
    let mut fixture = Fixture::new().await;
    let mut spec = tests::spec();
    spec.home.mode = HomeVolumeMode::NativeVctEmptyV1;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    let claim = assigned_spec(&fixture, &spec).await;
    fixture.client.config.native_claim_guard_enabled = true;
    fixture.state.lock().unwrap().rows.insert(POD.into(), json!({
        "apiVersion":"v1", "kind":"Pod",
        "metadata":{"name":"warm-pod", "namespace":"cma-test", "uid":"pod-uid",
            "labels":{"cma/control-plane":"cma-test", "agents.x-k8s.io/sandbox-name-hash":super::super::pods::name_hash("warm-member")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox", "name":"warm-member", "uid":"member-uid", "controller":true}]},
        "status":{"phase":"Running", "podIP":"10.0.0.7",
            "conditions":[{"type":"Ready", "status":if ready {"True"} else {"False"}}],
            "containerStatuses":[{"name":"runtime", "image":"image", "imageID":"image-id", "ready":ready, "restartCount":12,
                "state":if ready {json!({"running":{}})} else {json!({"waiting":{"reason":"CrashLoopBackOff"}})}}]}
    }));
    (fixture, claim)
}

#[tokio::test]
async fn crash_loop_is_read_only_and_retains_the_assigned_claim_and_home() {
    let (fixture, claim) = fixture(false).await;
    let before = fixture.state.lock().unwrap().rows.clone();
    fixture.state.lock().unwrap().calls.clear();
    let error = fixture.client.adopt_home(&claim).await.unwrap_err();
    assert_eq!(
        error,
        AdapterError::NativeClaimNotReady(super::super::NativeClaimGuardReason::CrashLoop)
    );
    assert_eq!(fixture.state.lock().unwrap().rows, before);
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
    assert_eq!(
        fixture.client.observe_claim(&claim).await.unwrap().uid,
        claim.uid
    );
}

#[tokio::test]
async fn healthy_guard_adopts_once_and_recovery_preserves_the_home() {
    let (fixture, claim) = fixture(true).await;
    let home = fixture.client.adopt_home(&claim).await.unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0]["state"] =
            json!({"waiting":{"reason":"CrashLoopBackOff"}});
        state.calls.clear();
    }
    assert!(matches!(
        fixture.client.adopt_home(&claim).await,
        Err(AdapterError::NativeClaimNotReady(_))
    ));
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
    fixture.state.lock().unwrap().rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0]
        ["state"] = json!({"running":{}});
    assert_eq!(fixture.client.adopt_home(&claim).await.unwrap(), home);
}

#[tokio::test]
async fn disabled_guard_does_not_probe_the_pod() {
    let (mut fixture, claim) = fixture(false).await;
    fixture.client.config.native_claim_guard_enabled = false;
    fixture.state.lock().unwrap().calls.clear();
    fixture.client.adopt_home(&claim).await.unwrap();
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| !call.contains("/pods")));
}

#[tokio::test]
async fn create_binding_checks_the_same_owned_pod_before_assignment_delivery() {
    let (fixture, _) = fixture(false).await;
    let member: kube::api::DynamicObject = {
        let mut state = fixture.state.lock().unwrap();
        let member = state.rows.get_mut(MEMBER).unwrap();
        member["metadata"]["labels"] = json!({"cma/control-plane":"cma-test"});
        serde_json::from_value(member.clone()).unwrap()
    };
    let client = super::super::AgentSandboxClient::new(
        fixture.kubernetes.clone(),
        fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    fixture.state.lock().unwrap().calls.clear();
    let error = client
        .wait_ready_for_create(&super::super::model::handle(&member).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        AdapterError::NativeClaimNotReady(super::super::NativeClaimGuardReason::CrashLoop)
    );
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
    assert!(fixture.state.lock().unwrap().assignments.is_empty());
}

#[tokio::test]
async fn foreign_pod_health_cannot_reject_the_assigned_runtime_as_crash_looping() {
    let (fixture, claim) = fixture(false).await;
    fixture.state.lock().unwrap().rows.get_mut(POD).unwrap()["metadata"]["ownerReferences"][0]
        ["uid"] = json!("foreign-member");
    fixture.state.lock().unwrap().calls.clear();
    let error = fixture.client.adopt_home(&claim).await.unwrap_err();
    assert_eq!(
        error,
        AdapterError::NativeClaimNotReady(super::super::NativeClaimGuardReason::TimedOut)
    );
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}

#[tokio::test]
async fn changed_claim_and_member_owner_are_rejected_without_effects() {
    for change in ["claim_uid", "assignment", "member_owner"] {
        let (fixture, claim) = fixture(true).await;
        match change {
            "claim_uid" => fixture.edit("sandboxclaims", &claim.name, |value| {
                value["metadata"]["uid"] = json!("replacement")
            }),
            "assignment" => fixture.edit("sandboxclaims", &claim.name, |value| {
                value["status"]["sandbox"]["name"] = json!("replacement")
            }),
            _ => {
                fixture.state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["metadata"]
                    ["ownerReferences"][0]["uid"] = json!("foreign")
            }
        }
        fixture.state.lock().unwrap().calls.clear();
        assert!(
            matches!(
                fixture.client.adopt_home(&claim).await,
                Err(AdapterError::Rejected(_))
            ),
            "{change}"
        );
        assert!(fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(call, _)| call.starts_with("GET ")));
    }
}

#[tokio::test]
async fn authority_acknowledgement_timeout_keeps_published_home_for_replay() {
    let (fixture, claim) = fixture(true).await;
    fixture.state.lock().unwrap().native_annotation_generation = true;
    fixture.state.lock().unwrap().calls.clear();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(4),
        fixture.client.adopt_home(&claim),
    )
    .await
    .expect("authority observation must not wait five seconds")
    .unwrap_err();
    let outcome = not_ready_outcome(&error);
    assert!(handle.render().lines().any(|line| {
        line.starts_with("cma_native_claim_guard_total{")
            && line.contains("stage=\"home_authority\"")
            && line.contains(&format!("outcome=\"{outcome}\""))
            && line.ends_with(" 1")
    }));
    let original_authority = fixture.row("sandboxclaims", &claim.name)["metadata"]["annotations"]
        [NATIVE_HOME_ANNOTATION]
        .clone();
    assert!(original_authority.is_string());
    {
        let mut state = fixture.state.lock().unwrap();
        assert_eq!(
            state
                .calls
                .iter()
                .filter(|(call, _)| !call.starts_with("GET "))
                .count(),
            2
        );
        let member = state.rows.get_mut(MEMBER).unwrap();
        member["status"]["conditions"][0]["observedGeneration"] =
            member["metadata"]["generation"].clone();
        state.calls.clear();
    }
    let home = fixture.client.adopt_home(&claim).await.unwrap();
    assert_eq!(
        home.receipt.native_authority.as_ref().unwrap().claim.uid,
        claim.uid
    );
    assert_eq!(
        fixture.row("sandboxclaims", &claim.name)["metadata"]["annotations"]
            [NATIVE_HOME_ANNOTATION],
        original_authority
    );
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}
