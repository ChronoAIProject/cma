use super::*;
use crate::agent_sandbox::{RetainedStartupPolicyPredecessor, RetainedStartupPolicyUpgrade};

const PODS: &str = "/api/v1/namespaces/cma-test/pods";

async fn predecessor(fixture: &Resume) -> crate::Result<RetainedStartupPolicyPredecessor> {
    client(&fixture.fixture, true)
        .observe_retained_startup_policy_predecessor(
            &fixture.handle,
            &fixture.previous,
            &fixture.home,
        )
        .await
}

async fn dispatch(
    fixture: &Resume,
    admission: &RetainedStartupPolicyUpgrade<'_>,
) -> crate::Result<()> {
    client(&fixture.fixture, true)
        .upgrade_retained_startup_policy(
            &fixture.handle,
            &fixture.previous,
            &fixture.home,
            admission,
        )
        .await
}

#[tokio::test]
async fn retained_policy_upgrade_rejects_a_changed_predecessor_version() {
    let fixture = Resume::new().await;
    let observed = predecessor(&fixture).await.unwrap();
    assert_eq!(observed.generation, 1);
    assert_eq!(observed.image, fixture.previous.image.uri);
    assert_eq!(observed.home_identity, fixture.home.identity().unwrap());
    assert!(!observed.journal_recovery);
    fixture.no_writes();
    let admission = RetainedStartupPolicyUpgrade {
        operation_id: "retained-policy-operation",
        predecessor: &observed,
        target_image: &fixture.previous.image.uri,
        successor_generation: 2,
        journal_recovery: true,
    };
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(MEMBER)
        .unwrap()["metadata"]["resourceVersion"] = json!("42");
    let error = dispatch(&fixture, &admission).await.unwrap_err();
    assert!(matches!(error, crate::AdapterError::Rejected(message)
        if message == "retained startup policy predecessor changed after admission"));
    fixture.no_writes();
}

#[tokio::test]
async fn retained_policy_upgrade_replays_an_exact_lost_ack_after_version_advance() {
    let fixture = Resume::new().await;
    let observed = predecessor(&fixture).await.unwrap();
    let mut admission = RetainedStartupPolicyUpgrade {
        operation_id: "retained-policy-operation",
        predecessor: &observed,
        target_image: &fixture.previous.image.uri,
        successor_generation: 2,
        journal_recovery: true,
    };
    fixture
        .fixture
        .state
        .lock()
        .unwrap()
        .lose
        .push(format!("PATCH {MEMBER}"));
    assert!(dispatch(&fixture, &admission).await.is_err());
    {
        let mut state = fixture.fixture.state.lock().unwrap();
        let row = state.rows.get_mut(MEMBER).unwrap();
        let proof: serde_json::Value = serde_json::from_str(
            row["metadata"]["annotations"]["cma/native-runtime-upgrade"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(proof["version"], 2);
        assert_eq!(
            proof["retained_policy"]["operation_id"],
            admission.operation_id
        );
        assert_eq!(proof["retained_policy"]["journal_recovery"], true);
        assert_eq!(
            proof["retained_policy"]["predecessor_resource_version"],
            observed.resource_version
        );
        row["metadata"]["resourceVersion"] = json!("42");
        state.calls.clear();
    }
    dispatch(&fixture, &admission).await.unwrap();
    fixture.no_writes();
    let mut other_predecessor = observed.clone();
    other_predecessor.resource_version = "42".into();
    admission.predecessor = &other_predecessor;
    assert!(dispatch(&fixture, &admission).await.is_err());
    fixture.no_writes();
    admission.predecessor = &observed;
    admission.operation_id = "different-operation";
    assert!(dispatch(&fixture, &admission).await.is_err());
    fixture.no_writes();
}

#[tokio::test]
async fn retained_policy_refuses_an_owned_pod_even_without_its_tracking_label() {
    use crate::agent_sandbox::pods::TRACKING_LABEL;
    const POD: &str = "/api/v1/namespaces/cma-test/pods/warm-pod";
    let fixture = Resume::new().await;
    let observed = predecessor(&fixture).await.unwrap();
    let admission = RetainedStartupPolicyUpgrade {
        operation_id: "retained-policy-operation",
        predecessor: &observed,
        target_image: &fixture.previous.image.uri,
        successor_generation: 2,
        journal_recovery: true,
    };
    for labels in [json!({}), json!({(TRACKING_LABEL): "different"})] {
        fixture.fixture.state.lock().unwrap().rows.insert(
            POD.into(),
            json!({"apiVersion":"v1","kind":"Pod","metadata":{
                "name":"warm-pod","namespace":"cma-test","uid":"pod-uid",
                "labels": labels,
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1",
                    "kind":"Sandbox","name":"warm-member","uid":observed.uid}]
            }}),
        );
        let observation = predecessor(&fixture).await.unwrap_err();
        let mutation = dispatch(&fixture, &admission).await.unwrap_err();
        for error in [observation, mutation] {
            assert!(matches!(error, crate::AdapterError::Unavailable(message)
                if message == "native member Pods remain present"));
        }
        fixture.no_writes();
    }
    fixture.fixture.state.lock().unwrap().rows.remove(POD);
    dispatch(&fixture, &admission).await.unwrap();
}

#[tokio::test]
async fn retained_policy_observation_refuses_a_concurrent_member_edit() {
    use super::super::read_fixture::Effects;

    let fixture = Resume::new().await;
    let gate = Gate::new();
    let effects = Effects::new(
        &fixture.fixture,
        BTreeMap::from([(PODS.into(), gate.clone())]),
    );
    let adapter = AgentSandboxClient::new(
        effects.client.clone(),
        fixture.fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let observe = adapter.observe_retained_startup_policy_predecessor(
        &fixture.handle,
        &fixture.previous,
        &fixture.home,
    );
    let edit = async {
        gate.wait().await;
        fixture
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .get_mut(MEMBER)
            .unwrap()["metadata"]["resourceVersion"] = json!("42");
        gate.respond();
    };
    let (result, ()) = tokio::join!(observe, edit);
    let error = result.unwrap_err();
    assert!(matches!(error, crate::AdapterError::Unavailable(message)
        if message == "stopped Sandbox changed during observation"));
    effects.assert_read_only();
}

#[tokio::test]
async fn retained_policy_observation_rejects_an_incomplete_pod_inventory() {
    let fixture = Resume::new().await;
    let (_, adapter) = fixture.reads(
        BTreeMap::from([(
            PODS.into(),
            (
                200,
                json!({"apiVersion":"v1","kind":"PodList","metadata":{
                    "resourceVersion":"5","continue":"repeated-page"
                },"items":[]}),
            ),
        )]),
        BTreeMap::new(),
    );
    let error = adapter
        .observe_retained_startup_policy_predecessor(
            &fixture.handle,
            &fixture.previous,
            &fixture.home,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, crate::AdapterError::Unavailable(_)));
}

#[tokio::test]
async fn retained_policy_upgrade_refuses_a_different_frozen_request() {
    let fixture = Resume::new().await;
    let mut observed = predecessor(&fixture).await.unwrap();
    observed.request_sha256 = "a".repeat(64);
    let admission = RetainedStartupPolicyUpgrade {
        operation_id: "retained-policy-operation",
        predecessor: &observed,
        target_image: &fixture.previous.image.uri,
        successor_generation: 2,
        journal_recovery: true,
    };
    let error = dispatch(&fixture, &admission).await.unwrap_err();
    assert!(matches!(error, crate::AdapterError::Rejected(message)
        if message == "retained startup policy predecessor differs from admission"));
    fixture.no_writes();
}
