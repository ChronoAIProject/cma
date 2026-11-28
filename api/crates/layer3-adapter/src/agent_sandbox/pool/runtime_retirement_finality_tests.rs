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
async fn verified_retirement_requires_suspension_and_absence_of_unlabeled_owned_pods() {
    let (fixture, home, request, handle) = bound(true).await;
    let client = client(&fixture, false);
    assert!(client
        .retire_suspended(&handle, &request, &home)
        .await
        .is_err());
    let pod = "/api/v1/namespaces/cma-test/pods/retiring-original";
    {
        let mut state = fixture.state.lock().unwrap();
        assert!(!state
            .calls
            .iter()
            .any(|(call, _)| call.starts_with("DELETE ")));
        state.rows.insert(
            pod.into(),
            json!({
                "apiVersion":"v1", "kind":"Pod", "metadata":{
                    "name":"retiring-original", "namespace":"cma-test", "uid":"original-pod",
                    "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                        "name":"warm-member","uid":"member-uid","controller":true}]
                },
                "status":{"phase":"Running"}
            }),
        );
    }
    client.suspend(&handle, &request).await.unwrap();
    for outcome in [
        client.verify_retirement_suspension(&handle, &request).await,
        client.retire_suspended(&handle, &request, &home).await,
    ] {
        assert!(matches!(
            outcome,
            Err(AdapterError::Unavailable(reason)) if reason == "native member Pods remain present"
        ));
    }
    {
        let mut state = fixture.state.lock().unwrap();
        assert!(!state
            .calls
            .iter()
            .any(|(call, _)| call.starts_with("DELETE ")));
        state.rows.remove(pod).unwrap();
    }
    client
        .verify_retirement_suspension(&handle, &request)
        .await
        .unwrap();
    client
        .retire_suspended(&handle, &request, &home)
        .await
        .unwrap();
    let state = fixture.state.lock().unwrap();
    let (_, delete) = state
        .calls
        .iter()
        .find(|(call, _)| call == &format!("DELETE {MEMBER}"))
        .unwrap();
    assert_eq!(delete["preconditions"]["uid"], "member-uid");
    assert!(delete["preconditions"]["resourceVersion"].is_string());
    assert_eq!(delete["propagationPolicy"], "Foreground");
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
            if native {
                {
                    let mut state = fixture.state.lock().unwrap();
                    let pvc = state.rows.get_mut(VOLUME).unwrap()["metadata"]
                        .as_object_mut()
                        .unwrap();
                    pvc.remove("labels");
                    pvc.remove("ownerReferences");
                }
                assert!(matches!(
                    client(&fixture, false).verify_retired(&handle, &home).await,
                    Err(AdapterError::Unavailable(reason)) if reason == "native retirement PVCs remain present"
                ));
                fixture.state.lock().unwrap().rows.remove(VOLUME).unwrap();
            }
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
            assert_eq!(state.rows.contains_key(VOLUME), !native);
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
async fn fresh_finality_waits_for_owned_pods_regardless_of_tracking_label() {
    let (fixture, home, request, handle) = bound(true).await;
    let runtime = client(&fixture, false);
    runtime
        .retire_verified(&handle, &request, &home, None)
        .await
        .unwrap();
    let path = "/api/v1/namespaces/cma-test/pods/retiring-original";
    let tracking = super::super::super::pods::TRACKING_LABEL;
    for labels in [
        json!({}),
        json!({tracking: "changed"}),
        json!({tracking: super::super::super::pods::name_hash("warm-member")}),
    ] {
        fixture.state.lock().unwrap().rows.insert(path.into(), json!({
            "apiVersion":"v1", "kind":"Pod", "metadata":{
                "name":"retiring-original", "namespace":"cma-test", "uid":"original-pod",
                "deletionTimestamp":"2026-01-01T00:00:00Z",
                "labels":labels,
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]
            }
        }));
        fixture.state.lock().unwrap().calls.clear();
        assert!(tokio::time::timeout(
            Duration::from_millis(80),
            runtime.verify_retired(&handle, &home)
        )
        .await
        .is_err());
        let state = fixture.state.lock().unwrap();
        assert!(state.calls.iter().any(|(call, _)| call.ends_with("/pods")));
        assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
    }
    fixture.state.lock().unwrap().rows.remove(path);
    fixture.state.lock().unwrap().rows.remove(VOLUME).unwrap();
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
            let mut state = fixture.state.lock().unwrap();
            state.rows.remove(&pod_path).unwrap();
            state.rows.remove(VOLUME).unwrap();
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
    assert!(!state.rows.contains_key(VOLUME));
    assert!(state.calls[before_restart..]
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}

#[tokio::test]
async fn finality_rejects_replacements_and_incomplete_observations() {
    for change in ["runtime", "claim", "home", "pod_page", "runtime_page"] {
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
                "home" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement");
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

#[tokio::test]
async fn finality_checks_owned_pods_on_later_pages() {
    let (fixture, home, request, handle) = bound(true).await;
    let runtime = client(&fixture, false);
    runtime
        .retire_verified(&handle, &request, &home, None)
        .await
        .unwrap();

    let pods = "/api/v1/namespaces/cma-test/pods";
    let pod = json!({
        "apiVersion":"v1", "kind":"Pod", "metadata":{
            "name":"survivor", "namespace":"cma-test", "uid":"survivor-uid",
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                "name":"warm-member","uid":"member-uid","controller":true}]
        }
    });
    let page = json!({
        "apiVersion":"v1", "kind":"PodList",
        "metadata":{"resourceVersion":"100"}, "items":[]
    });
    let mut first = page.clone();
    first["metadata"]["continue"] = json!("next");
    let mut second = page;
    second["items"] = json!([pod.clone()]);
    {
        let mut state = fixture.state.lock().unwrap();
        state.calls.clear();
        state.rows.remove(VOLUME).unwrap();
        state.rows.insert(format!("{pods}/survivor"), pod);
        state
            .read_replies
            .extend([(pods.into(), 200, first), (pods.into(), 200, second)]);
    }

    let second_page = fixture.gate(format!("GET {pods}"), None);
    let probe = runtime.verify_retired(&handle, &home);
    tokio::pin!(probe);
    tokio::select! {
        result = &mut probe => panic!("finality resolved before the Pod inventory: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(5), second_page.entered.notified()) => result.unwrap(),
    }
    second_page.release.notify_one();
    assert!(tokio::time::timeout(Duration::from_millis(200), &mut probe)
        .await
        .is_err());
    {
        let mut state = fixture.state.lock().unwrap();
        assert!(state.read_replies.is_empty());
        assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
        state.rows.remove(&format!("{pods}/survivor"));
    }
    tokio::time::timeout(Duration::from_secs(5), probe)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn finality_reads_every_pvc_page_and_refuses_a_label_less_owned_survivor() {
    let (fixture, home, request, handle) = bound(true).await;
    let runtime = client(&fixture, false);
    runtime
        .retire_verified(&handle, &request, &home, None)
        .await
        .unwrap();
    const PVCS: &str = "/api/v1/namespaces/cma-test/persistentvolumeclaims";
    let pvc = json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": {
            "name": "extra-volume",
            "namespace": "cma-test",
            "uid": "extra-volume-uid",
            "deletionTimestamp": "2026-01-01T00:00:00Z",
            "ownerReferences": [{
                "apiVersion": "agents.x-k8s.io/v1beta1", "kind": "Sandbox",
                "name": "warm-member", "uid": "member-uid", "controller": true
            }]
        }
    });
    let mut unrelated = pvc.clone();
    unrelated["metadata"]["name"] = json!("sibling-volume");
    unrelated["metadata"]["uid"] = json!("sibling-volume-uid");
    unrelated["metadata"]["ownerReferences"][0]["uid"] = json!("sibling-member-uid");
    let last = json!({
        "apiVersion": "v1", "kind": "PersistentVolumeClaimList",
        "metadata": {"resourceVersion": "100"}, "items": []
    });
    let mut first = last.clone();
    first["metadata"]["continue"] = json!("next");
    first["items"] = json!([unrelated]);
    {
        let mut state = fixture.state.lock().unwrap();
        state.rows.remove(VOLUME).unwrap();
        state.calls.clear();
        state.list_queries.clear();
    }
    for (resource_version, items, expected_refusal) in [
        (
            "100",
            json!([pvc]),
            Some("native retirement PVCs remain present"),
        ),
        (
            "101",
            json!([]),
            Some("native cleanup inventory changed between pages"),
        ),
        ("100", json!([]), None),
    ] {
        let mut second = last.clone();
        second["metadata"]["resourceVersion"] = json!(resource_version);
        second["items"] = items;
        fixture
            .state
            .lock()
            .unwrap()
            .list_replies
            .extend([(200, first.clone()), (200, second)]);
        let result = runtime.verify_retired(&handle, &home).await;
        match expected_refusal {
            Some(expected) => assert!(matches!(
                result,
                Err(AdapterError::Unavailable(reason)) if reason == expected
            )),
            None => result.unwrap(),
        }
    }
    let state = fixture.state.lock().unwrap();
    assert!(state.list_replies.is_empty());
    assert_eq!(state.list_queries.len(), 6);
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(call, _)| call.strip_prefix("GET ") == Some(PVCS))
            .count(),
        6
    );
    assert!(state
        .list_queries
        .iter()
        .all(|query| !query.contains("labelSelector")));
    assert!(state
        .list_queries
        .iter()
        .skip(1)
        .step_by(2)
        .all(|query| query.contains("continue=next")));
    assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
}

#[tokio::test]
async fn unreachable_retirement_tracks_three_probes_and_rejects_replacement_uid() {
    use crate::hosted::{HostedControlTransport, ProviderHostedControlTransport};
    use crate::owner_protocol::{TransportOwner, OWNER_PROTOCOL_VERSION};

    let (fixture, home, request, handle) = bound(true).await;
    let runtime = client(&fixture, true);
    let transport = ProviderHostedControlTransport::for_agent_sandbox(runtime.clone()).unwrap();
    let pods_path = "/api/v1/namespaces/cma-test/pods";
    let pod_path = format!("{pods_path}/retiring-original");
    let owner = TransportOwner {
        protocol: OWNER_PROTOCOL_VERSION,
        agent_id: "fixture-agent".into(),
        instance_id: "fixture-manager".into(),
        epoch: 1,
    };
    for status in [
        json!({"phase":"Pending"}),
        json!({"phase":"Running","conditions":[{"type":"Ready","status":"False"}]}),
        json!({"phase":"Running","conditions":[{"type":"Ready","status":"True"}]}),
        json!({
            "phase":"Running", "podIP":"192.0.2.1",
            "conditions":[{"type":"Ready","status":"True"}],
            "containerStatuses":[{"name":"runtime","image":"fixture","imageID":"fixture",
                "ready":false,"restartCount":4,"state":{"waiting":{"reason":"CrashLoopBackOff"}}}]
        }),
    ] {
        let mut state = fixture.state.lock().unwrap();
        state.rows.insert(pod_path.clone(), json!({
            "apiVersion":"v1","kind":"Pod",
            "metadata":{
                "name":"retiring-original","namespace":"cma-test","uid":"original-pod",
                "labels":{
                    OWNER_LABEL:"cma-test",
                    super::super::super::pods::TRACKING_LABEL:super::super::super::pods::name_hash("warm-member")
                },
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                    "name":"warm-member","uid":"member-uid","controller":true}]
            },
            "status":status
        }));
        state.calls.clear();
        drop(state);
        let pod = transport
            .probe_unreachable_receiver(&handle, &request, &home, &owner, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pod, crate::RetirementPod::by_uid("original-pod".into()));
        let state = fixture.state.lock().unwrap();
        assert_eq!(
            state
                .calls
                .iter()
                .filter(|(call, _)| call == &format!("GET {pods_path}"))
                .count(),
            6
        );
        assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
    }

    fixture.state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["spec"]["podTemplate"]["spec"]
        ["serviceAccountName"] = json!("older-runtime-account");
    let original = fixture.state.lock().unwrap().rows[&pod_path].clone();
    let mut replacement = original.clone();
    replacement["metadata"]["uid"] = json!("replacement");
    let list = |pod| json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[pod]});
    {
        let mut state = fixture.state.lock().unwrap();
        state.read_replies.extend([
            (pods_path.into(), 200, list(original.clone())),
            (pods_path.into(), 200, list(original)),
            (pods_path.into(), 200, list(replacement.clone())),
        ]);
    }
    assert!(transport
        .probe_unreachable_receiver(&handle, &request, &home, &owner, None)
        .await
        .is_err());
    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .insert(pod_path.clone(), replacement);
    assert!(runtime
        .retire_verified(
            &handle,
            &request,
            &home,
            Some(&crate::RetirementPod::by_uid("original-pod".into())),
        )
        .await
        .is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));

    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(&pod_path)
        .unwrap()["metadata"]["uid"] = json!("original-pod");
    let proved = transport
        .probe_unreachable_receiver(&handle, &request, &home, &owner, None)
        .await
        .unwrap()
        .unwrap();
    assert!(runtime.validate_request(&handle, &request).await.is_err());
    let deleted = fixture.gate(format!("GET {pods_path}"), Some(format!("DELETE {MEMBER}")));
    let retirement = runtime.retire_verified(&handle, &request, &home, Some(&proved));
    tokio::pin!(retirement);
    let deadline = Duration::from_secs(5);
    tokio::select! {
        result = &mut retirement => panic!("provider completed before Pod inventory: {result:?}"),
        result = tokio::time::timeout(deadline, deleted.entered.notified()) => result.unwrap(),
    }
    assert!(tokio::time::timeout(
        Duration::from_millis(80),
        runtime.verify_retired(&handle, &home)
    )
    .await
    .is_err());
    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .remove(&pod_path)
        .unwrap();
    deleted.release.notify_one();
    tokio::time::timeout(deadline, retirement)
        .await
        .unwrap()
        .unwrap();
    assert!(runtime.verify_retired(&handle, &home).await.is_err());
    fixture.state.lock().unwrap().rows.remove(VOLUME).unwrap();
    runtime.verify_retired(&handle, &home).await.unwrap();
    let state = fixture.state.lock().unwrap();
    let (_, deletion) = state
        .calls
        .iter()
        .find(|(call, _)| call == &format!("DELETE {MEMBER}"))
        .unwrap();
    assert_eq!(deletion["preconditions"]["uid"], "member-uid");
    assert!(deletion["preconditions"]["resourceVersion"].is_string());
}
