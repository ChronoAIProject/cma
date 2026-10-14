use super::{
    fixture::{prefix, Fixture},
    home_tests::{assigned_spec, MEMBER, VOLUME},
    network_policy_tests,
    read_fixture::{failure, Effects, Gate, Reads},
    tests, NativePoolClaim, NativePoolSpec,
};
use crate::{AdapterError, HomeVolumeMode};
use kube::ResourceExt;
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

fn missing() -> (u16, serde_json::Value) {
    let (status, mut body) = failure(404);
    body["reason"] = json!("NotFound");
    (status, body)
}

fn spec() -> NativePoolSpec {
    let mut spec = tests::spec();
    spec.home.mode = HomeVolumeMode::NativeVctFullHomeV2;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    spec.image.labels.insert(
        crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
        "2".into(),
    );
    spec
}
async fn fixture() -> Fixture {
    let mut fixture = Fixture::new().await;
    fixture.client.config.template_network_policy =
        Some(network_policy_tests::policy("10.254.0.0/16"));
    fixture
}
fn paths(claim: &NativePoolClaim) -> [String; 3] {
    [
        format!("{}/sandboxclaims/{}", prefix(), claim.name),
        format!(
            "{}/sandboxtemplates/{}",
            prefix(),
            claim.request.pool.template_name
        ),
        format!("{}/sandboxwarmpools/{}", prefix(), claim.request.pool.name),
    ]
}

#[tokio::test]
async fn native_claim_reads_overlap_and_create_waits_for_both_orders() {
    for claim_first in [true, false] {
        let fixture = fixture().await;
        let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
        let request = tests::request(binding.clone());
        let name = super::model::claim(&fixture.client.config, &request)
            .unwrap()
            .name_any();
        let paths = [
            format!("{}/sandboxclaims/{name}", prefix()),
            format!("{}/sandboxtemplates/{}", prefix(), binding.template_name),
            format!("{}/sandboxwarmpools/{}", prefix(), binding.name),
        ];
        let gates: BTreeMap<_, _> = paths.iter().map(|p| (p.clone(), Gate::new())).collect();
        let effects = Effects::new(&fixture, gates.clone());
        let pools = effects.pools(&fixture);
        let mut task = tokio::spawn(async move { pools.claim(&request).await });
        for gate in gates.values() {
            gate.wait().await;
        }
        for (i, path) in paths.iter().enumerate() {
            if (i == 0) == claim_first {
                gates[path].respond();
            }
        }
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        effects.assert_read_only();
        for (i, path) in paths.iter().enumerate() {
            if (i == 0) != claim_first {
                gates[path].respond();
            }
        }
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.name, name);
        assert_eq!(
            effects
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.starts_with("POST "))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn native_claim_replay_ignores_policy_failure_but_retains_claim_error_precedence() {
    let fixture = fixture().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let paths = paths(&claim);
    let reads = Reads::new(
        &fixture,
        BTreeMap::from([(paths[1].clone(), failure(429))]),
        BTreeMap::new(),
    );
    assert_eq!(
        reads.pools(&fixture).claim(&claim.request).await.unwrap(),
        claim
    );
    for (claim_reply, capacity) in [(failure(403), false), (missing(), true)] {
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([
                (paths[0].clone(), claim_reply),
                (paths[1].clone(), failure(429)),
            ]),
            BTreeMap::new(),
        );
        let error = reads
            .pools(&fixture)
            .claim(&claim.request)
            .await
            .unwrap_err();
        assert_eq!(
            matches!(error, AdapterError::AtCapacity(_)),
            capacity,
            "{error:?}"
        );
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_claim_missing_refuses_changed_pool_before_create() {
    let fixture = fixture().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let paths = paths(&claim);
    for path in &paths[1..] {
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([(paths[0].clone(), missing()), (path.clone(), failure(403))]),
            BTreeMap::new(),
        );
        assert!(matches!(
            reads.pools(&fixture).claim(&claim.request).await,
            Err(AdapterError::Rejected(_))
        ));
    }
}

#[tokio::test]
async fn native_home_policy_and_graph_overlap_but_both_gate_ordered_publication() {
    for policy_first in [true, false] {
        let fixture = fixture().await;
        let claim = assigned_spec(&fixture, &spec()).await;
        let paths = paths(&claim);
        let all = [
            paths[0].clone(),
            paths[1].clone(),
            paths[2].clone(),
            MEMBER.into(),
            VOLUME.into(),
        ];
        let gates: BTreeMap<_, _> = all.iter().map(|p| (p.clone(), Gate::new())).collect();
        let effects = Effects::new(&fixture, gates.clone());
        let pools = effects.pools(&fixture);
        let original = claim.clone();
        let mut task = tokio::spawn(async move { pools.adopt_home(&original).await });
        for path in &paths {
            gates[path].wait().await;
        }
        gates[&paths[0]].respond();
        gates[MEMBER].wait().await;
        gates[VOLUME].wait().await;
        for (i, path) in all.iter().enumerate().skip(1) {
            if (i < 3) == policy_first {
                gates[path].respond();
            }
        }
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        effects.assert_read_only();
        for (i, path) in all.iter().enumerate().skip(1) {
            if (i < 3) != policy_first {
                gates[path].respond();
            }
        }
        let home = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(home.receipt.storage_generation, "pvc-uid");
        let calls = effects.calls.lock().unwrap();
        let writes: Vec<_> = calls
            .iter()
            .filter(|call| !call.starts_with("GET "))
            .collect();
        assert_eq!(
            writes,
            [&format!("PATCH {}", paths[0]), &format!("PATCH {MEMBER}")]
        );
        for path in [&paths[0], MEMBER, VOLUME] {
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| **call == format!("GET {path}"))
                    .count(),
                2,
                "fresh post-publication proof"
            );
        }
    }
}

#[tokio::test]
async fn native_home_refuses_either_read_branch_and_keeps_policy_error_precedence() {
    let fixture = fixture().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let paths = paths(&claim);
    for path in [
        paths[0].as_str(),
        paths[1].as_str(),
        paths[2].as_str(),
        MEMBER,
        VOLUME,
    ] {
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([(path.into(), failure(403))]),
            BTreeMap::new(),
        );
        assert!(matches!(
            reads.pools(&fixture).adopt_home(&claim).await,
            Err(AdapterError::Rejected(_))
        ));
    }
    for policy_first in [true, false] {
        let gates: BTreeMap<_, _> = paths.iter().map(|p| (p.clone(), Gate::new())).collect();
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([
                (paths[0].clone(), failure(403)),
                (paths[1].clone(), failure(429)),
            ]),
            gates.clone(),
        );
        let pools = reads.pools(&fixture);
        let original = claim.clone();
        let mut task = tokio::spawn(async move { pools.adopt_home(&original).await });
        for gate in gates.values() {
            gate.wait().await;
        }
        for (i, path) in paths.iter().enumerate() {
            if (i != 0) == policy_first {
                gates[path].respond();
            }
        }
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        for (i, path) in paths.iter().enumerate() {
            if (i != 0) != policy_first {
                gates[path].respond();
            }
        }
        assert!(matches!(
            task.await.unwrap(),
            Err(AdapterError::AtCapacity(_))
        ));
        assert_eq!(
            reads.calls.lock().unwrap().len(),
            3,
            "invalid Claim cannot address member or PVC"
        );
    }
}

#[tokio::test]
async fn native_home_keeps_original_resource_version_conflict_fence() {
    let fixture = fixture().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let claim_path = paths(&claim)[0].clone();
    fixture
        .state
        .lock()
        .unwrap()
        .conflict
        .push(format!("PATCH {claim_path}"));
    fixture.state.lock().unwrap().calls.clear();
    assert!(fixture.client.adopt_home(&claim).await.is_err());
    let state = fixture.state.lock().unwrap();
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(call, _)| call.starts_with("PATCH "))
            .count(),
        1
    );
    assert_eq!(state.rows[MEMBER]["metadata"]["annotations"], json!(null));
}
