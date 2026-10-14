use super::*;
use std::time::Duration;

async fn pending_fixture() -> (Fixture, NativePoolClaim, usize) {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let floor = {
        let mut state = fixture.state.lock().unwrap();
        state.native_annotation_generation = true;
        state.calls.len()
    };
    (fixture, claim, floor)
}

async fn observed_pending(fixture: &Fixture, floor: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let reads = fixture.state.lock().unwrap().calls[floor..]
                .iter()
                .filter(|(call, _)| call == &format!("GET {MEMBER}"))
                .count();
            // The adoption read, its first acknowledgement, and one poll.
            if reads >= 3 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("original member must be observed while waiting");
}

fn writes_once(fixture: &Fixture, floor: usize, claim: &NativePoolClaim) {
    let state = fixture.state.lock().unwrap();
    let calls = &state.calls[floor..];
    let writes: Vec<_> = calls
        .iter()
        .filter(|(call, _)| !call.starts_with("GET "))
        .map(|(call, _)| call.clone())
        .collect();
    assert_eq!(
        writes,
        vec![
            format!(
                "PATCH {}/sandboxclaims/{}",
                super::super::fixture::prefix(),
                claim.name
            ),
            format!("PATCH {MEMBER}"),
        ]
    );
    assert!(calls
        .iter()
        .all(|(call, _)| !call.contains("persistentvolumeclaims") || call.starts_with("GET ")));
}

#[tokio::test]
async fn native_home_waits_for_annotation_generation_in_one_adoption_without_replaying_writes() {
    let (fixture, claim, floor) = pending_fixture().await;
    let client = fixture.client.clone();
    let original = claim.clone();
    let adopting = tokio::spawn(async move { client.adopt_home(&original).await });
    observed_pending(&fixture, floor).await;
    assert!(!adopting.is_finished());
    {
        let mut state = fixture.state.lock().unwrap();
        let member = state.rows.get_mut(MEMBER).unwrap();
        assert_eq!(member["metadata"]["generation"], 2);
        assert_eq!(member["status"]["conditions"][0]["observedGeneration"], 1);
        member["status"]["conditions"][0]["observedGeneration"] = json!(2);
    }
    let home = tokio::time::timeout(Duration::from_secs(2), adopting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let origin = home.receipt.native_authority.unwrap();
    assert_eq!(origin.claim, claim);
    assert_eq!(origin.sandbox_uid, "member-uid");
    assert_eq!(origin.pvc_uid, "pvc-uid");
    writes_once(&fixture, floor, &claim);
}

#[tokio::test]
async fn native_home_pending_observation_rejects_authority_changes_without_replaying_adoption() {
    for change in [
        "claim_uid",
        "claim_assignment",
        "claim_origin",
        "member_uid",
        "member_owner",
        "member_origin",
        "pvc_uid",
    ] {
        let (fixture, claim, floor) = pending_fixture().await;
        let client = fixture.client.clone();
        let original = claim.clone();
        let adopting = tokio::spawn(async move { client.adopt_home(&original).await });
        observed_pending(&fixture, floor).await;
        {
            let mut state = fixture.state.lock().unwrap();
            let path = format!(
                "{}/sandboxclaims/{}",
                super::super::fixture::prefix(),
                claim.name
            );
            match change {
                "claim_uid" => {
                    state.rows.get_mut(&path).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                "claim_assignment" => {
                    state.rows.get_mut(&path).unwrap()["status"]["sandbox"]["name"] =
                        json!("replacement")
                }
                "claim_origin" => {
                    state.rows.get_mut(&path).unwrap()["metadata"]["annotations"]
                        [NATIVE_HOME_ANNOTATION] = json!("{}")
                }
                "member_uid" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                "member_owner" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["ownerReferences"][0]["uid"] =
                        json!("replacement")
                }
                "member_origin" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["annotations"]
                        [NATIVE_HOME_ANNOTATION] = json!("{}")
                }
                "pvc_uid" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                _ => unreachable!(),
            }
        }
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(2), adopting)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(crate::AdapterError::Rejected(_))
            ),
            "{change}"
        );
        writes_once(&fixture, floor, &claim);
    }
}

#[tokio::test]
async fn native_home_pending_observation_propagates_dependency_errors_immediately() {
    let (fixture, claim, floor) = pending_fixture().await;
    let client = fixture.client.clone();
    let original = claim.clone();
    let adopting = tokio::spawn(async move { client.adopt_home(&original).await });
    observed_pending(&fixture, floor).await;
    fixture.state.lock().unwrap().rows.remove(MEMBER);
    let error = tokio::time::timeout(Duration::from_secs(2), adopting)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(!error
        .to_string()
        .contains("initialization is still pending"));
    writes_once(&fixture, floor, &claim);
}
