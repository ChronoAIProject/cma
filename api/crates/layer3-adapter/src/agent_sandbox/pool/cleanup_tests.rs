use super::{
    fixture::Fixture,
    home_tests::{adopted, assigned, MEMBER, VOLUME},
    *,
};
use serde_json::json;

const POD: &str = "/api/v1/namespaces/cma-test/pods/warm-pod";

fn plan(claim: &NativePoolClaim) -> NativePoolCleanup {
    NativePoolCleanup {
        request: claim.request.clone(),
        claim: Some(claim.clone()),
        members: Vec::new(),
    }
}

fn pod(fixture: &Fixture, member: &str, uid: &str) {
    fixture.state.lock().unwrap().rows.insert(POD.into(), json!({"apiVersion":"v1","kind":"Pod",
        "metadata":{"name":"warm-pod","namespace":"cma-test","uid":"pod-uid","resourceVersion":"52",
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":member,"uid":uid,"controller":true}]}}));
}

#[tokio::test]
async fn unadmitted_cleanup_records_member_before_delete_and_waits_for_orphan_pods_before_pvc() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    pod(&fixture, "warm-member", "member-uid");
    let initial = plan(&claim);
    assert!(fixture.client.cleanup(&initial).await.is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| !call.starts_with("DELETE ")));
    let planned = fixture.client.plan_cleanup(&initial).await.unwrap();
    assert_eq!(
        planned.members,
        [NativePoolMember {
            pvc_uid: None,
            name: "warm-member".into(),
            uid: "member-uid".into()
        }]
    );
    let restored: NativePoolCleanup =
        serde_json::from_slice(&serde_json::to_vec(&planned).unwrap()).unwrap();
    assert!(fixture.client.cleanup(&restored).await.is_err());
    {
        let state = fixture.state.lock().unwrap();
        let deletes: Vec<_> = state
            .calls
            .iter()
            .filter(|(call, _)| call.starts_with("DELETE "))
            .collect();
        assert!(deletes[0].0.ends_with(&claim.name));
        assert_eq!(deletes[1].0, format!("DELETE {MEMBER}"));
        assert!(state.rows.contains_key(VOLUME));
    }
    fixture.state.lock().unwrap().rows.remove(POD);
    fixture.client.cleanup(&restored).await.unwrap();
    assert!(!fixture.state.lock().unwrap().rows.contains_key(VOLUME));
    fixture.client.cleanup(&restored).await.unwrap();
}

#[tokio::test]
async fn lost_claim_receipt_is_recovered_without_creation_and_retained_disk_goes_to_home_store() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    let initial = NativePoolCleanup {
        request: claim.request.clone(),
        claim: None,
        members: Vec::new(),
    };
    let before = fixture.state.lock().unwrap().calls.len();
    let planned = fixture.client.plan_cleanup(&initial).await.unwrap();
    assert_eq!(planned.claim.as_ref().unwrap().uid, claim.uid);
    fixture.lost("DELETE", "sandboxclaims");
    assert!(fixture.client.cleanup(&planned).await.is_err());
    fixture.client.cleanup(&planned).await.unwrap();
    assert!(fixture.state.lock().unwrap().rows.contains_key(VOLUME));
    assert!(fixture.state.lock().unwrap().calls[before..]
        .iter()
        .all(|(call, _)| !call.starts_with("POST ")));
    use crate::HomeVolumeStore;
    home_tests::homes(&fixture)
        .remove(&home.request, None)
        .await
        .unwrap();
    assert!(!fixture.state.lock().unwrap().rows.contains_key(VOLUME));
}

#[tokio::test]
async fn missing_claim_does_not_hide_unreported_member_or_replaced_claim_member_and_pvc() {
    for change in ["unreported", "claim", "member", "volume", "orphan"] {
        let fixture = Fixture::new().await;
        let mut claim = assigned(&fixture).await;
        if change == "unreported" {
            fixture.edit("sandboxclaims", &claim.name, |row| {
                row["status"] = json!({});
            });
            claim.sandbox_name = None;
        }
        let planned = fixture.client.plan_cleanup(&plan(&claim)).await.unwrap();
        match change {
            "claim" => fixture.edit("sandboxclaims", &claim.name, |row| {
                row["metadata"]["uid"] = json!("replacement")
            }),
            "member" => {
                fixture.state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["metadata"]["uid"] =
                    json!("replacement")
            }
            "volume" => {
                fixture.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]
                    ["ownerReferences"][0]["uid"] = json!("replacement")
            }
            "orphan" => pod(&fixture, "unknown-member", "unknown-uid"),
            _ => (),
        }
        let result = fixture.client.cleanup(&planned).await;
        if change == "unreported" {
            result.unwrap();
        } else {
            assert!(result.is_err(), "{change}");
            assert!(fixture.state.lock().unwrap().rows.contains_key(VOLUME));
        }
    }
}

#[tokio::test]
async fn never_created_claim_can_finish_without_allocating_or_touching_another_live_member() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    pod(&fixture, "warm-member", "member-uid");
    let mut other = claim.request.clone();
    other.home.sandbox_id = "other-sandbox".into();
    let empty = NativePoolCleanup {
        request: other,
        claim: None,
        members: Vec::new(),
    };
    let before = fixture.state.lock().unwrap().calls.len();
    assert_eq!(fixture.client.plan_cleanup(&empty).await.unwrap(), empty);
    fixture.client.cleanup(&empty).await.unwrap();
    assert!(fixture.state.lock().unwrap().calls[before..]
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}

#[tokio::test]
async fn late_orphan_claim_persists_cleanup_authority_and_retained_pvc_still_waits_for_pods() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    pod(&fixture, "warm-member", "member-uid");
    let inventory = fixture.client.inventory_claims().await.unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].uid, claim.uid);
    let before = fixture.state.lock().unwrap().calls.len();
    assert!(fixture.client.reclaim_claim(&inventory[0]).await.is_err());
    {
        let state = fixture.state.lock().unwrap();
        let calls = &state.calls[before..];
        let recorded = calls
            .iter()
            .position(|(call, body)| {
                call.starts_with("PATCH ")
                    && body["metadata"]["annotations"]["cma/native-cleanup"].is_string()
            })
            .unwrap();
        let deleted = calls
            .iter()
            .position(|(call, _)| call.starts_with("DELETE "))
            .unwrap();
        assert!(recorded < deleted);
    }
    assert!(fixture.client.inventory_claims().await.unwrap().is_empty());
    assert!(fixture.client.cleanup_retained_home(&home).await.is_err());
    fixture.state.lock().unwrap().rows.remove(POD);
    fixture.client.cleanup_retained_home(&home).await.unwrap();
    use crate::HomeVolumeStore;
    home_tests::homes(&fixture)
        .remove(&home.request, Some(&home.receipt))
        .await
        .unwrap();
}
