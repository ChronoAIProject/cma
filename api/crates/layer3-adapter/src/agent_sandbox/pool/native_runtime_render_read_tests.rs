//! Runtime rendering overlaps reads without changing the live transition proof.
use super::*;
use crate::agent_sandbox::pool::{
    fixture::prefix,
    read_fixture::{failure, Gate, Reads},
    runtime_tests::request,
};
use crate::AdapterError;
use std::{collections::BTreeMap, time::Duration};

#[tokio::test]
async fn native_runtime_render_reads_overlap_and_reacquire_all_authority() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let origin = home.receipt.native_authority.as_deref().unwrap();
    let request = request(&home, 1);
    let expected = fixture
        .client
        .runtime_desired(origin, &request, "warm-member")
        .await
        .unwrap();
    let paths = [
        format!("{}/sandboxclaims/{}", prefix(), claim.name),
        VOLUME.into(),
        MEMBER.into(),
    ];
    for _ in 0..2 {
        let gates: BTreeMap<_, _> = paths.iter().map(|p| (p.clone(), Gate::new())).collect();
        let reads = Reads::new(&fixture, BTreeMap::new(), gates.clone());
        let pools = reads.pools(&fixture);
        let origin = origin.clone();
        let request = request.clone();
        let task = tokio::spawn(async move {
            pools
                .runtime_desired(&origin, &request, "warm-member")
                .await
        });
        for gate in gates.values() {
            gate.wait().await;
        }
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
        assert!(!task.is_finished());
        // A faster member/PVC cannot complete rendering before Claim authority.
        gates[MEMBER].respond();
        gates[VOLUME].respond();
        assert!(!task.is_finished());
        gates[&paths[0]].respond();
        let rendered = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(rendered, expected);
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_runtime_render_preserves_claim_then_volume_then_member_error_order() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let origin = home.receipt.native_authority.as_deref().unwrap();
    let request = request(&home, 1);
    let claim_path = format!("{}/sandboxclaims/{}", prefix(), claim.name);
    for (first, later) in [
        (claim_path.as_str(), VOLUME),
        (VOLUME, MEMBER),
        (MEMBER, MEMBER),
    ] {
        let gate = Gate::new();
        let mut overrides = BTreeMap::from([(later.into(), failure(403))]);
        overrides.insert(first.into(), failure(429));
        let reads = Reads::new(
            &fixture,
            overrides,
            BTreeMap::from([(first.into(), gate.clone())]),
        );
        let pools = reads.pools(&fixture);
        let origin = origin.clone();
        let request = request.clone();
        let task = tokio::spawn(async move {
            pools
                .runtime_desired(&origin, &request, "warm-member")
                .await
        });
        gate.wait().await;
        assert!(!task.is_finished(), "earlier authority remains pending");
        gate.respond();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
            Err(AdapterError::AtCapacity(_))
        ));
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_runtime_render_refuses_replaced_authority_before_later_api_errors() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let origin = home.receipt.native_authority.as_deref().unwrap();
    let request = request(&home, 1);
    let claim_path = format!("{}/sandboxclaims/{}", prefix(), claim.name);
    for (path, pointer, later) in [
        (claim_path.as_str(), "/metadata/uid", Some(VOLUME)),
        (claim_path.as_str(), "/status/sandbox/name", Some(VOLUME)),
        (VOLUME, "/metadata/uid", Some(MEMBER)),
        (VOLUME, "/metadata/ownerReferences/0/uid", Some(MEMBER)),
        (MEMBER, "/metadata/uid", None),
        (MEMBER, "/metadata/ownerReferences/0/uid", None),
    ] {
        let mut replacement = fixture.state.lock().unwrap().rows[path].clone();
        *replacement.pointer_mut(pointer).unwrap() = json!("replacement");
        let mut overrides = BTreeMap::from([(path.into(), (200, replacement))]);
        if let Some(later) = later {
            overrides.insert(later.into(), failure(429));
        }
        let reads = Reads::new(&fixture, overrides, BTreeMap::new());
        assert!(
            matches!(
                reads
                    .pools(&fixture)
                    .runtime_desired(origin, &request, "warm-member")
                    .await,
                Err(AdapterError::Rejected(_))
            ),
            "{path} {pointer}"
        );
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
    }
}

#[tokio::test]
async fn native_runtime_render_classifies_member_image_drift_after_authority_reads() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let origin = home.receipt.native_authority.as_deref().unwrap();
    let request = request(&home, 1);
    let mut member = fixture.state.lock().unwrap().rows[MEMBER].clone();
    member["spec"]["podTemplate"]["spec"]["containers"][0]["image"] =
        json!("example.invalid/runtime:drifted");
    let reads = Reads::new(
        &fixture,
        BTreeMap::from([(MEMBER.into(), (200, member))]),
        BTreeMap::new(),
    );
    assert!(matches!(
        reads
            .pools(&fixture)
            .runtime_desired(origin, &request, "warm-member")
            .await,
        Err(AdapterError::AdmittedSandboxSpecDrift)
    ));
    assert_eq!(reads.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn native_runtime_render_rejects_invalid_local_origin_before_resource_reads() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let request = request(&home, 1);
    for change in ["scope", "claim", "name"] {
        let mut origin = home.receipt.native_authority.as_deref().unwrap().clone();
        let name = if change == "name" {
            "foreign"
        } else {
            "warm-member"
        };
        match change {
            "scope" => origin.claim.request.pool.config_sha256 = "0".repeat(64),
            "claim" => origin.claim.name = "foreign".into(),
            "name" => {}
            _ => unreachable!(),
        }
        let reads = Reads::new(&fixture, BTreeMap::new(), BTreeMap::new());
        assert!(reads
            .pools(&fixture)
            .runtime_desired(&origin, &request, name)
            .await
            .is_err());
        assert!(reads.calls.lock().unwrap().is_empty(), "{change}");
    }
}
