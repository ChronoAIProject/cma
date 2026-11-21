use super::*;
use crate::agent_sandbox::pool::{
    fixture::prefix,
    read_fixture::{failure, Gate, Reads},
};
use std::{collections::BTreeMap, time::Duration};

#[tokio::test]
async fn native_adoption_publishes_original_claim_then_member_with_fresh_final_proof() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let claim_path = format!("{}/sandboxclaims/{}", prefix(), claim.name);
    let claim_version =
        fixture.row("sandboxclaims", &claim.name)["metadata"]["resourceVersion"].clone();
    fixture.state.lock().unwrap().calls.clear();
    fixture.client.adopt_home(&claim).await.unwrap();
    let state = fixture.state.lock().unwrap();
    let calls = &state.calls;
    assert_eq!(
        calls.len(),
        8,
        "three reads, two ordered writes, three fresh reads"
    );
    assert_eq!(calls[0].0, format!("GET {claim_path}"));
    assert!(calls[1..3]
        .iter()
        .all(|(call, _)| { call == &format!("GET {MEMBER}") || call == &format!("GET {VOLUME}") }));
    assert_eq!(calls[3].0, format!("PATCH {claim_path}"));
    assert_eq!(calls[3].1["metadata"]["uid"], claim.uid);
    assert_eq!(calls[3].1["metadata"]["resourceVersion"], claim_version);
    assert_eq!(calls[4].0, format!("PATCH {MEMBER}"));
    assert_eq!(calls[4].1["metadata"]["uid"], "member-uid");
    assert_eq!(calls[4].1["metadata"]["resourceVersion"], "50");
    for path in [&claim_path, MEMBER, VOLUME] {
        assert_eq!(
            calls[5..]
                .iter()
                .filter(|(call, _)| call == &format!("GET {path}"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn native_adoption_reads_claim_once_then_member_and_pvc_together() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let claim_path = format!("{}/sandboxclaims/{}", prefix(), claim.name);
    let claim_gate = Gate::new();
    let member_gate = Gate::new();
    let volume_gate = Gate::new();
    let reads = Reads::new(
        &fixture,
        BTreeMap::new(),
        BTreeMap::from([
            (claim_path.clone(), claim_gate.clone()),
            (MEMBER.into(), member_gate.clone()),
            (VOLUME.into(), volume_gate.clone()),
        ]),
    );
    let pools = reads.pools(&fixture);
    let task = tokio::spawn(async move { pools.adopt_home(&claim).await });
    claim_gate.wait().await;
    assert_eq!(
        reads.calls.lock().unwrap().as_slice(),
        std::slice::from_ref(&claim_path)
    );
    claim_gate.respond();
    member_gate.wait().await;
    volume_gate.wait().await;
    {
        let calls = reads.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls.iter().filter(|p| *p == &claim_path).count(), 1);
    }
    assert!(!task.is_finished());
    member_gate.respond();
    volume_gate.respond();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        home
    );
    let calls = reads.calls.lock().unwrap();
    assert_eq!(calls.len(), 6, "three initial reads plus fresh final proof");
    for path in [&claim_path, MEMBER, VOLUME] {
        assert_eq!(calls.iter().filter(|p| p.as_str() == path).count(), 2);
    }
}

#[tokio::test]
async fn native_adoption_invalid_claim_never_reads_dependents() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let claim_path = format!("{}/sandboxclaims/{}", prefix(), claim.name);
    for change in [
        "api",
        "uid",
        "assignment",
        "overrides",
        "deleting",
        "unassigned",
    ] {
        let mut row = fixture.row("sandboxclaims", &claim.name);
        match change {
            "uid" => row["metadata"]["uid"] = json!("replacement"),
            "assignment" => row["status"]["sandbox"]["name"] = json!("other-member"),
            "overrides" => row["spec"]["env"] = json!([{"name":"CHANGED","value":"value"}]),
            "deleting" => row["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z"),
            "unassigned" => row["status"] = json!({}),
            _ => (),
        }
        let reply = if change == "api" {
            failure(403)
        } else {
            (200, row)
        };
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([(claim_path.clone(), reply)]),
            BTreeMap::new(),
        );
        assert!(
            reads.pools(&fixture).adopt_home(&claim).await.is_err(),
            "{change}"
        );
        assert_eq!(
            reads.calls.lock().unwrap().as_slice(),
            std::slice::from_ref(&claim_path),
            "{change}"
        );
    }
}

#[tokio::test]
async fn native_adoption_member_error_precedes_faster_pvc_failure() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    adopted(&fixture, &claim).await;
    for change in ["api", "owner", "deleting", "version"] {
        let mut member = fixture.state.lock().unwrap().rows[MEMBER].clone();
        match change {
            "owner" => member["metadata"]["ownerReferences"][0]["uid"] = json!("replacement"),
            "deleting" => member["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z"),
            "version" => member["metadata"]["resourceVersion"] = json!(""),
            _ => (),
        }
        let member_reply = if change == "api" {
            failure(429)
        } else {
            (200, member)
        };
        let baseline = Reads::new(
            &fixture,
            BTreeMap::from([(MEMBER.into(), member_reply.clone())]),
            BTreeMap::new(),
        );
        let expected = baseline
            .pools(&fixture)
            .adopt_home(&claim)
            .await
            .unwrap_err();
        let member_gate = Gate::new();
        let volume_gate = Gate::new();
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([(MEMBER.into(), member_reply), (VOLUME.into(), failure(403))]),
            BTreeMap::from([
                (MEMBER.into(), member_gate.clone()),
                (VOLUME.into(), volume_gate.clone()),
            ]),
        );
        let pools = reads.pools(&fixture);
        let original = claim.clone();
        let mut task = tokio::spawn(async move { pools.adopt_home(&original).await });
        member_gate.wait().await;
        volume_gate.wait().await;
        volume_gate.respond();
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        member_gate.respond();
        let actual = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "{change}");
        assert_eq!(reads.calls.lock().unwrap().len(), 3);
    }
}
