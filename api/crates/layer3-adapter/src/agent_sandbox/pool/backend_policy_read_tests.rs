use super::*;
use crate::agent_sandbox::pool::{
    fixture::prefix,
    read_fixture::{failure, Gate, Reads},
};
use std::{collections::BTreeMap, time::Duration};

const PODS: &str = "/api/v1/namespaces/cma-test/pods";

fn reads_for(
    fixture: &InspectionFixture,
    mut overrides: BTreeMap<String, (u16, serde_json::Value)>,
    gates: BTreeMap<String, Arc<Gate>>,
) -> (Reads, AgentSandboxBackend) {
    let pod = fixture.bound.fixture.state.lock().unwrap().rows[POD].clone();
    overrides.entry(PODS.into()).or_insert_with(|| {
        (200, json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":[pod]}))
    });
    let reads = Reads::new(&fixture.bound.fixture, overrides, gates);
    let client = AgentSandboxClient::new(
        reads.client.clone(),
        fixture.bound.fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let backend = AgentSandboxBackend::new(client, fixture.transport.clone());
    (reads, backend)
}

#[tokio::test]
async fn native_policy_reads_overlap_all_four_graph_reads() {
    let fixture = inspection_fixture(true).await;
    let binding = &fixture.bound.claim.request.pool;
    let paths = [
        format!("{}/sandboxclaims/{}", prefix(), fixture.bound.claim.name),
        MEMBER.into(),
        VOLUME.into(),
        PODS.into(),
        format!("{}/sandboxtemplates/{}", prefix(), binding.template_name),
        format!("{}/sandboxwarmpools/{}", prefix(), binding.name),
    ];
    let gates: BTreeMap<_, _> = paths.into_iter().map(|p| (p, Gate::new())).collect();
    let (reads, backend) = reads_for(&fixture, BTreeMap::new(), gates.clone());
    let handle = fixture.bound.runtime.clone();
    let request = fixture.bound.request.clone();
    let home = fixture.home.clone();
    let task = tokio::spawn(async move {
        backend
            .inspect_native_runtime(&handle, &request, &home)
            .await
    });
    // No read can complete until the entire graph and policy wave has started.
    for gate in gates.values() {
        gate.wait().await;
    }
    assert!(!task.is_finished());
    for gate in gates.values() {
        gate.respond();
    }
    assert!(tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .is_none());
    assert_eq!(reads.calls.lock().unwrap().len(), 6);
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_policy_failure_preserves_graph_error_and_missing_ack_precedence() {
    let fixture = inspection_fixture(true).await;
    let template = format!(
        "{}/sandboxtemplates/{}",
        prefix(),
        fixture.bound.claim.request.pool.template_name
    );
    let claim = format!("{}/sandboxclaims/{}", prefix(), fixture.bound.claim.name);
    for graph_failure in [false, true] {
        let mut overrides = BTreeMap::from([(template.clone(), failure(429))]);
        if graph_failure {
            overrides.insert(claim.clone(), failure(403));
        }
        let graph_gate = Gate::new();
        let (reads, backend) = reads_for(
            &fixture,
            overrides,
            BTreeMap::from([(claim.clone(), graph_gate.clone())]),
        );
        let handle = fixture.bound.runtime.clone();
        let request = fixture.bound.request.clone();
        let home = fixture.home.clone();
        let mut task = tokio::spawn(async move {
            backend
                .inspect_native_runtime(&handle, &request, &home)
                .await
        });
        graph_gate.wait().await;
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        assert_eq!(reads.calls.lock().unwrap().len(), 6);
        graph_gate.respond();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        if graph_failure {
            assert!(matches!(result, Err(AdapterError::Rejected(_))));
        } else {
            assert!(result.unwrap().is_none());
        }
        assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn native_policy_rejects_malformed_local_authority_before_any_reads() {
    let fixture = inspection_fixture(true).await;
    for change in ["handle", "uid", "receipt", "scope", "origin", "digest"] {
        let mut handle = fixture.bound.runtime.clone();
        let mut home = fixture.home.clone();
        match change {
            "handle" => handle = RuntimeHandle::new("malformed"),
            "uid" => handle = RuntimeHandle::new("agent-sandbox:warm-member:replacement"),
            "receipt" => home.receipt.storage_generation = "replacement".into(),
            "scope" => home.request.scope = None,
            "origin" => home.receipt.native_authority = None,
            "digest" => {
                home.receipt
                    .native_authority
                    .as_mut()
                    .unwrap()
                    .claim
                    .request
                    .pool
                    .config_sha256 = "f".repeat(64)
            }
            _ => unreachable!(),
        }
        let (reads, backend) = reads_for(&fixture, BTreeMap::new(), BTreeMap::new());
        assert!(
            backend
                .inspect_native_runtime(&handle, &fixture.bound.request, &home)
                .await
                .is_err(),
            "{change}"
        );
        assert!(reads.calls.lock().unwrap().is_empty(), "{change}");
        assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 0);
    }
}
