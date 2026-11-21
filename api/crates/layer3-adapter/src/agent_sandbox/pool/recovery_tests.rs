use super::{
    fixture::Fixture,
    home_tests::{adopted, assigned, homes, MEMBER, VOLUME},
    *,
};
use crate::HomeVolumeStore;
use serde_json::{json, Value};

fn page(items: Vec<Value>, cursor: &str) -> (u16, Value) {
    (
        200,
        json!({"apiVersion":"v1", "kind":"PersistentVolumeClaimList",
        "metadata":{"continue":cursor}, "items":items}),
    )
}

#[tokio::test]
async fn lost_attachment_recovers_original_disk_after_claim_and_seed_disappear() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    fixture.client.delete_claim(&claim).await.unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        // Model completed foreground GC; storage recovery does not certify Pod finality.
        state.rows.remove(MEMBER);
        state.rows.remove(super::fixture::seed_path());
    }
    assert_eq!(
        fixture.client.recover_home(&claim).await.unwrap(),
        Some(home.clone())
    );
    let mut replaced = claim.clone();
    replaced.uid = "replacement-claim".into();
    assert!(fixture.client.recover_home(&replaced).await.is_err());
    homes(&fixture).remove(&home.request, None).await.unwrap();
    assert_eq!(fixture.client.recover_home(&claim).await.unwrap(), None);
    homes(&fixture).remove(&home.request, None).await.unwrap();
    let state = fixture.state.lock().unwrap();
    assert!(!state.rows.contains_key(VOLUME));
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(c, _)| c == &format!("DELETE {VOLUME}"))
            .count(),
        1
    );
    assert!(!state
        .calls
        .iter()
        .any(|(c, _)| c.starts_with("POST /api/v1/")));
}

#[tokio::test]
async fn recovery_reads_all_pages_and_refuses_uncertain_or_duplicate_inventory_before_deletion() {
    for case in ["complete", "duplicate", "cycle", "forbidden", "bound"] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        let home = adopted(&fixture, &claim).await;
        {
            let mut state = fixture.state.lock().unwrap();
            let volume = state.rows[VOLUME].clone();
            state
                .list_replies
                .push_back(page(vec![volume.clone()], "page-2"));
            match case {
                "complete" => state.list_replies.push_back(page(vec![], "")),
                "duplicate" => state.list_replies.push_back(page(vec![volume], "")),
                "cycle" => state.list_replies.push_back(page(vec![], "page-2")),
                "forbidden" => state.list_replies.push_back((
                    403,
                    json!({
                    "apiVersion":"v1","kind":"Status","status":"Failure",
                    "reason":"Forbidden","message":"fixture","code":403}),
                )),
                _ => {
                    for index in 3..=65 {
                        state
                            .list_replies
                            .push_back(page(vec![], &format!("page-{index}")));
                    }
                }
            }
        }
        let result = homes(&fixture).remove(&home.request, None).await;
        assert_eq!(result.is_ok(), case == "complete", "{case}");
        let state = fixture.state.lock().unwrap();
        assert_eq!(
            state
                .calls
                .iter()
                .any(|(c, _)| c == &format!("DELETE {VOLUME}")),
            case == "complete"
        );
        assert!(state
            .list_queries
            .iter()
            .all(|q| q.contains("labelSelector=") && q.contains("limit=200")));
        assert!(state.list_queries[1].contains("continue=page-2"));
        assert!(state.list_queries.len() <= 64);
    }
}

#[tokio::test]
async fn recovery_refuses_changed_origin_storage_and_conflicting_ordinary_allocation() {
    use sha2::{Digest, Sha256};
    for case in ["uid", "origin", "policy", "pool_config", "ordinary"] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        let home = adopted(&fixture, &claim).await;
        {
            let mut state = fixture.state.lock().unwrap();
            let volume = state.rows.get_mut(VOLUME).unwrap();
            match case {
                "uid" => volume["metadata"]["uid"] = json!("other-disk"),
                "origin" => {
                    volume["metadata"]["annotations"]
                        .as_object_mut()
                        .unwrap()
                        .remove(NATIVE_HOME_ANNOTATION);
                }
                "policy" => volume["spec"]["resources"]["requests"]["storage"] = json!("128Gi"),
                "pool_config" => {
                    let mut origin: NativePoolHomeOrigin = serde_json::from_str(
                        volume["metadata"]["annotations"][NATIVE_HOME_ANNOTATION]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                    origin.claim.request.pool.pod_policy.service_account = "different".into();
                    volume["metadata"]["annotations"][NATIVE_HOME_ANNOTATION] =
                        json!(serde_json::to_string(&origin).unwrap());
                }
                _ => {
                    let mut ordinary = volume.clone();
                    let name = format!(
                        "cma-home-{}",
                        &format!("{:x}", Sha256::digest(home.request.volume_id.as_bytes()))[..52]
                    );
                    ordinary["metadata"]["name"] = json!(name);
                    ordinary["metadata"]["uid"] = json!("other-disk");
                    ordinary["metadata"]["annotations"]
                        .as_object_mut()
                        .unwrap()
                        .remove(NATIVE_HOME_ANNOTATION);
                    ordinary["metadata"]["labels"]
                        .as_object_mut()
                        .unwrap()
                        .remove(NATIVE_HOME_REQUEST_LABEL);
                    state.rows.insert(
                        format!("/api/v1/namespaces/cma-test/persistentvolumeclaims/{name}"),
                        ordinary,
                    );
                }
            }
        }
        assert!(
            homes(&fixture).remove(&home.request, None).await.is_err(),
            "{case}"
        );
        assert!(!fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(c, _)| c.starts_with("DELETE ")));
    }
}
