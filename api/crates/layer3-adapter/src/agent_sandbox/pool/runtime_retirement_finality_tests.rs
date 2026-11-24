use super::super::runtime_tests::{client, request};
use super::*;
use crate::{AdapterError, HomeVolumeAttachment, RuntimeHandle};
use std::{sync::atomic::Ordering, time::Duration};

async fn bound(
    native: bool,
) -> (
    Fixture,
    HomeVolumeAttachment,
    crate::provider::ProviderSandboxRequest,
    RuntimeHandle,
) {
    let fixture = Fixture::new().await;
    let policy = if native {
        spec()
    } else {
        super::super::tests::spec()
    };
    let claim = assigned_spec(&fixture, &policy).await;
    let home = adopted(&fixture, &claim).await;
    let request = request(&home, 1);
    let handle = client(&fixture, false)
        .admit_pool_runtime(&claim, &home, &request)
        .await
        .unwrap();
    fixture.state.lock().unwrap().calls.clear();
    (fixture, home, request, handle)
}

#[tokio::test]
async fn verified_retirement_retries_lost_deletion_without_reconstructing_a_receipt() {
    for native in [false, true] {
        for lost in [false, true] {
            let (fixture, home, request, handle) = bound(native).await;
            if lost {
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .lose
                    .push(format!("DELETE {MEMBER}"));
                assert!(client(&fixture, false)
                    .retire_verified(&handle, &request, &home, None)
                    .await
                    .is_err());
            }
            client(&fixture, false)
                .retire_verified(&handle, &request, &home, None)
                .await
                .unwrap();
            client(&fixture, false)
                .verify_retired(&handle, &home)
                .await
                .unwrap();
            // Restart after deletion, before aggregate settlement: exact proof is repeatable.
            client(&fixture, false)
                .retire_verified(&handle, &request, &home, None)
                .await
                .unwrap();
            client(&fixture, false)
                .verify_retired(&handle, &home)
                .await
                .unwrap();
            let state = fixture.state.lock().unwrap();
            assert!(!state.rows.contains_key(MEMBER));
            assert!(state.rows.contains_key(VOLUME));
            let deletes: Vec<_> = state
                .calls
                .iter()
                .filter(|(c, _)| c.starts_with("DELETE "))
                .collect();
            assert_eq!(deletes.len(), 2);
            assert!(deletes[0].0.contains("sandboxclaims/"));
            assert_eq!(deletes[1].0, format!("DELETE {MEMBER}"));
            assert_eq!(deletes[1].1["preconditions"]["uid"], "member-uid");
            assert!(deletes[1].1["preconditions"]["resourceVersion"].is_string());
            assert_eq!(deletes[1].1["propagationPolicy"], "Foreground");
            assert!(deletes[1].1.get("gracePeriodSeconds").is_none());
        }
    }
}

#[tokio::test]
async fn verified_retirement_refuses_changed_authority_without_provider_writes() {
    for native in [false, true] {
        for change in [
            "runtime_uid",
            "home_uid",
            "generation",
            "scope",
            "missing_home",
            "request_home",
            "namespace",
            "provider_denial",
        ] {
            let (fixture, mut home, mut request, handle) = bound(native).await;
            {
                let mut state = fixture.state.lock().unwrap();
                match change {
                    "runtime_uid" => state.rows.get_mut(MEMBER).unwrap()["metadata"]["uid"] = json!("replacement"),
                    "home_uid" => state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement"),
                    "generation" => { request.metadata.insert("cma/runtime-generation".into(), "2".into()); },
                    "scope" => home.request.scope = Some(crate::HomeVolumeScope::from_authority("foreign", &["foreign"]).unwrap()),
                    "missing_home" => { state.rows.remove(VOLUME); },
                    "request_home" => { request.metadata.insert("cma/home-identity".into(), "changed".into()); },
                    "namespace" => state.rows.get_mut(MEMBER).unwrap()["metadata"]["namespace"] = json!("foreign"),
                    "provider_denial" => state.read_replies.push_back((MEMBER.into(), 403,
                        json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Forbidden","code":403}))),
                    _ => unreachable!(),
                }
            }
            assert!(
                client(&fixture, false)
                    .retire_verified(&handle, &request, &home, None)
                    .await
                    .is_err(),
                "{native}/{change}"
            );
            assert!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .all(|(c, _)| c.starts_with("GET ")),
                "{native}/{change}"
            );
        }
    }
}

#[tokio::test]
async fn fresh_finality_waits_for_owned_pods_and_cancellation_never_writes() {
    let (fixture, home, request, handle) = bound(true).await;
    let runtime = client(&fixture, false);
    runtime
        .retire_verified(&handle, &request, &home, None)
        .await
        .unwrap();
    let path = "/api/v1/namespaces/cma-test/pods/retiring-original";
    fixture.state.lock().unwrap().rows.insert(path.into(), json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{
            "name":"retiring-original", "namespace":"cma-test", "uid":"original-pod",
            "deletionTimestamp":"2026-01-01T00:00:00Z",
            "labels":{super::super::super::pods::TRACKING_LABEL:super::super::super::pods::name_hash("warm-member")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]}}));
    fixture.state.lock().unwrap().calls.clear();
    assert!(tokio::time::timeout(
        Duration::from_millis(80),
        runtime.verify_retired(&handle, &home)
    )
    .await
    .is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(c, _)| c.starts_with("GET ")));
    fixture.state.lock().unwrap().rows.remove(path);
    runtime.verify_retired(&handle, &home).await.unwrap();
}

#[tokio::test]
async fn restarted_verifier_waits_for_original_pod_after_acknowledged_delete() {
    let (fixture, home, request, handle) = bound(true).await;
    const PODS: &str = "/api/v1/namespaces/cma-test/pods";
    let pod_path = format!("{PODS}/retiring-original");
    let original = fixture.state.lock().unwrap().rows[MEMBER].clone();
    fixture.state.lock().unwrap().rows.insert(pod_path.clone(), json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{
            "name":"retiring-original", "namespace":"cma-test", "uid":"original-pod",
            "deletionTimestamp":"2026-01-01T00:00:00Z",
            "labels":{super::super::super::pods::TRACKING_LABEL:super::super::super::pods::name_hash("warm-member")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]}}));
    let pod_read = format!("GET {PODS}");
    let acknowledged = fixture.gate(pod_read.clone(), Some(format!("DELETE {MEMBER}")));
    let deadline = Duration::from_secs(5);
    {
        let original_client = client(&fixture, false);
        let retirement = original_client.retire_verified(&handle, &request, &home, None);
        tokio::pin!(retirement);
        // A Pod read after the DELETE shows the client received the acknowledgement.
        tokio::select! {
            result = &mut retirement => panic!("retirement settled before Pod observation: {result:?}"),
            result = tokio::time::timeout(deadline, acknowledged.entered.notified()) => result.unwrap(),
        }
    }
    acknowledged.disconnect.store(true, Ordering::SeqCst);
    acknowledged.release.notify_one();
    let before_restart = {
        let state = fixture.state.lock().unwrap();
        assert!(!state.rows.contains_key(MEMBER));
        assert_eq!(state.rows[&pod_path]["metadata"]["uid"], "original-pod");
        state.calls.len()
    };

    let successor = client(&fixture, false);
    let first_read = fixture.gate(pod_read.clone(), None);
    let repeated_read = fixture.gate(pod_read, None);
    let finality = successor.verify_retired(&handle, &home);
    tokio::pin!(finality);
    for (gate, remove_pod) in [(&first_read, false), (&repeated_read, true)] {
        tokio::select! {
            result = &mut finality => panic!("finality settled with the original Pod present: {result:?}"),
            result = tokio::time::timeout(deadline, gate.entered.notified()) => result.unwrap(),
        }
        if remove_pod {
            fixture
                .state
                .lock()
                .unwrap()
                .rows
                .remove(&pod_path)
                .unwrap();
        }
        gate.release.notify_one();
    }
    tokio::time::timeout(deadline, finality)
        .await
        .unwrap()
        .unwrap();

    let mut replacement = original;
    replacement["metadata"]["uid"] = json!("replacement");
    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .insert(MEMBER.into(), replacement.clone());
    assert!(matches!(
        successor.verify_retired(&handle, &home).await,
        Err(AdapterError::Rejected(_))
    ));
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows[MEMBER], replacement);
    assert_eq!(
        state.rows[VOLUME]["metadata"]["uid"],
        home.receipt.storage_generation
    );
    assert!(state.calls[before_restart..]
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}

#[tokio::test]
async fn finality_rejects_replacements_and_incomplete_observations() {
    for change in ["runtime", "claim", "pod_page", "runtime_page"] {
        let (fixture, home, request, handle) = bound(true).await;
        let original = fixture.state.lock().unwrap().rows[MEMBER].clone();
        let origin = home.receipt.native_authority.as_deref().unwrap();
        let claim_path = format!(
            "{}/sandboxclaims/{}",
            super::super::fixture::prefix(),
            origin.claim.name
        );
        let original_claim = fixture.state.lock().unwrap().rows[&claim_path].clone();
        let runtime = client(&fixture, false);
        runtime
            .retire_verified(&handle, &request, &home, None)
            .await
            .unwrap();
        {
            let mut state = fixture.state.lock().unwrap();
            state.calls.clear();
            match change {
                "runtime" => {
                    let mut replacement = original;
                    replacement["metadata"]["uid"] = json!("replacement");
                    state.rows.insert(MEMBER.into(), replacement);
                }
                "claim" => {
                    let mut replacement = original_claim;
                    replacement["metadata"]["uid"] = json!("replacement");
                    state.rows.insert(claim_path, replacement);
                }
                "pod_page" | "runtime_page" => {
                    let complete = json!({"apiVersion":"v1","kind":"List","metadata":{"resourceVersion":"20"},"items":[]});
                    let mut incomplete = complete.clone();
                    incomplete["metadata"]["continue"] = json!("more");
                    if change == "pod_page" {
                        state.read_replies.push_back((
                            "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes".into(),
                            200,
                            complete,
                        ));
                    }
                    state.read_replies.push_back((
                        if change == "pod_page" {
                            "/api/v1/namespaces/cma-test/pods"
                        } else {
                            "/apis/agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxes"
                        }
                        .into(),
                        200,
                        incomplete,
                    ));
                }
                _ => unreachable!(),
            }
        }
        let result = runtime.verify_retired(&handle, &home).await;
        assert!(
            matches!(
                result,
                Err(AdapterError::Rejected(_) | AdapterError::Unavailable(_))
            ),
            "{change}: {result:?}"
        );
        assert!(fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|(c, _)| c.starts_with("GET ")));
    }
}
