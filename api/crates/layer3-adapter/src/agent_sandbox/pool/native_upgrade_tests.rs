use super::*;
use crate::agent_sandbox::pool::runtime_upgrade::UPGRADE_ANNOTATION;

fn target(
    home: &crate::HomeVolumeAttachment,
    generation: u64,
    image: &str,
) -> ProviderSandboxRequest {
    let mut next = request(home, generation);
    next.image.uri = image.into();
    next.metadata.insert(
        "cma/image-sha256".into(),
        crate::hosted::image_sha256(image),
    );
    resign(&mut next);
    next
}
fn resign(request: &mut ProviderSandboxRequest) {
    request.metadata.remove("cma/spec-sha256");
    request.metadata.insert(
        "cma/spec-sha256".into(),
        crate::hosted::provider_request_fingerprint(request).unwrap(),
    );
}
fn image(letter: char) -> String {
    format!(
        "registry.example/runtime@sha256:{}",
        letter.to_string().repeat(64)
    )
}
async fn upgrade(
    fixture: &Resume,
    previous: &ProviderSandboxRequest,
    next: &ProviderSandboxRequest,
) -> crate::Result<()> {
    client(&fixture.fixture, true)
        .upgrade_with_home(&fixture.handle, previous, next, &fixture.home)
        .await
}

#[tokio::test]
async fn native_upgrade_keeps_entire_original_graph_and_supports_recovery_resume_and_repeated_upgrades(
) {
    let fixture = Resume::new().await;
    let original = fixture.fixture.state.lock().unwrap().rows.clone();
    let next = target(&fixture.home, 2, &image('b'));
    upgrade(&fixture, &fixture.previous, &next).await.unwrap();
    {
        let state = fixture.fixture.state.lock().unwrap();
        for (path, row) in &original {
            if path != MEMBER {
                assert_eq!(&state.rows[path], row, "{path}");
            }
        }
        let mut expected = original[MEMBER]["spec"].clone();
        expected["operatingMode"] = json!("Running");
        expected["podTemplate"]["spec"]["containers"][0]["image"] = json!(next.image.uri);
        expected["podTemplate"]["spec"]["initContainers"][0]["image"] = json!(next.image.uri);
        assert_eq!(state.rows[MEMBER]["spec"], expected);
        let writes: Vec<_> = state
            .calls
            .iter()
            .filter(|(c, _)| !c.starts_with("GET "))
            .collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, format!("PATCH {MEMBER}"));
        assert_eq!(
            writes[0].1["metadata"]["uid"],
            original[MEMBER]["metadata"]["uid"]
        );
        assert_eq!(
            writes[0].1["metadata"]["resourceVersion"],
            original[MEMBER]["metadata"]["resourceVersion"]
        );
        assert_eq!(
            state.rows[MEMBER]["metadata"]["annotations"][NATIVE_HOME_ANNOTATION],
            original[MEMBER]["metadata"]["annotations"][NATIVE_HOME_ANNOTATION]
        );
    }
    let runtime = client(&fixture.fixture, true);
    runtime
        .validate_request(&fixture.handle, &next)
        .await
        .unwrap()
        .unwrap();
    assert!(runtime
        .validate_request(&fixture.handle, &fixture.previous)
        .await
        .is_err());
    assert_eq!(
        fixture
            .fixture
            .client
            .recover_known_native_vct(fixture.home.receipt.native_authority.as_deref().unwrap())
            .await
            .unwrap(),
        Some(fixture.home.clone())
    );
    let proof = fixture.fixture.state.lock().unwrap().rows[MEMBER]["metadata"]["annotations"]
        [UPGRADE_ANNOTATION]
        .clone();
    runtime.suspend(&fixture.handle, &next).await.unwrap();
    let resumed = target(&fixture.home, 3, &image('b'));
    runtime
        .resume_with_home(&fixture.handle, &next, &resumed, &fixture.home)
        .await
        .unwrap();
    assert_eq!(
        fixture.fixture.state.lock().unwrap().rows[MEMBER]["metadata"]["annotations"]
            [UPGRADE_ANNOTATION],
        proof
    );
    runtime.suspend(&fixture.handle, &resumed).await.unwrap();
    // Explicit rollback to the original image must keep proof too.
    let final_request = target(&fixture.home, 4, &fixture.previous.image.uri);
    upgrade(&fixture, &resumed, &final_request).await.unwrap();
    runtime
        .validate_request(&fixture.handle, &final_request)
        .await
        .unwrap()
        .unwrap();
    assert!(runtime
        .validate_request(&fixture.handle, &fixture.previous)
        .await
        .is_err());
    let encoded = fixture.fixture.state.lock().unwrap().rows[MEMBER]["metadata"]["annotations"]
        [UPGRADE_ANNOTATION]
        .as_str()
        .unwrap()
        .to_owned();
    let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert!(parsed["previous_proof_sha256"].as_str().is_some());
    assert_eq!(parsed["generation"], 4);
    assert_eq!(
        fixture.fixture.state.lock().unwrap().rows[VOLUME],
        original[VOLUME]
    );
}

#[tokio::test]
async fn native_upgrade_replays_lost_ack_and_conflicts_without_second_image_dispatch() {
    for lost in [false, true] {
        let fixture = Resume::new().await;
        let next = target(&fixture.home, 2, &image('b'));
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            if lost {
                &mut state.lose
            } else {
                &mut state.conflict
            }
            .push(format!("PATCH {MEMBER}"));
        }
        assert!(upgrade(&fixture, &fixture.previous, &next).await.is_err());
        fixture.fixture.state.lock().unwrap().calls.clear();
        upgrade(&fixture, &fixture.previous, &next).await.unwrap();
        let writes = fixture
            .fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(c, _)| c.starts_with("PATCH "))
            .count();
        assert_eq!(writes, usize::from(!lost));
        fixture.fixture.state.lock().unwrap().calls.clear();
        upgrade(&fixture, &fixture.previous, &next).await.unwrap();
        fixture.no_writes();
        let different = target(&fixture.home, 2, &image('c'));
        assert!(upgrade(&fixture, &fixture.previous, &different)
            .await
            .is_err());
        fixture.no_writes();
    }
}

#[tokio::test]
async fn native_upgrade_rejects_unrelated_changes_and_ordinary_resume_cannot_upgrade() {
    for change in [
        "generation",
        "resources",
        "mount",
        "timeout",
        "environment",
        "fingerprint",
        "metadata",
        "entrypoint",
        "image",
    ] {
        let fixture = Resume::new().await;
        let mut next = target(&fixture.home, 2, &image('b'));
        assert!(client(&fixture.fixture, true)
            .resume_with_home(&fixture.handle, &fixture.previous, &next, &fixture.home)
            .await
            .is_err());
        match change {
            "generation" => next = target(&fixture.home, 1, &image('b')),
            "resources" => {
                next.resource_limits.insert("cpu".into(), "9000m".into());
            }
            "mount" => next.volumes[0].read_only = true,
            "timeout" => next.timeout = Some(60),
            "environment" => {
                next.env.insert("USER".into(), "foreign".into());
            }
            "fingerprint" => {
                next.metadata
                    .insert("cma/spec-sha256".into(), "x".repeat(43));
            }
            "metadata" => {
                next.metadata
                    .insert("cma/native-runtime-upgrade".into(), "{}".into());
            }
            "entrypoint" => next.entrypoint.push("foreign".into()),
            "image" => next.image.uri = "registry.example/runtime:latest".into(),
            _ => unreachable!(),
        }
        if change != "fingerprint" {
            resign(&mut next);
        }
        assert!(
            upgrade(&fixture, &fixture.previous, &next).await.is_err(),
            "{change}"
        );
        fixture.no_writes();
    }
}

#[tokio::test]
async fn native_upgrade_rejects_live_drift_and_missing_or_changed_upgrade_proof() {
    for change in [
        "missing",
        "origin",
        "generation",
        "image",
        "request",
        "unknown",
        "oversized",
        "uid",
        "policy",
        "init_image",
        "pvc",
    ] {
        let fixture = Resume::new().await;
        let next = target(&fixture.home, 2, &image('b'));
        upgrade(&fixture, &fixture.previous, &next).await.unwrap();
        {
            let mut state = fixture.fixture.state.lock().unwrap();
            state.calls.clear();
            let member = state.rows.get_mut(MEMBER).unwrap();
            match change {
                "missing" => {
                    member["metadata"]["annotations"]
                        .as_object_mut()
                        .unwrap()
                        .remove(UPGRADE_ANNOTATION);
                }
                "uid" => member["metadata"]["uid"] = json!("replaced"),
                "policy" => {
                    member["spec"]["podTemplate"]["spec"]["serviceAccountName"] = json!("foreign")
                }
                "init_image" => {
                    member["spec"]["podTemplate"]["spec"]["initContainers"][0]["image"] =
                        json!(image('c'))
                }
                "pvc" => state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replaced"),
                "oversized" => {
                    member["metadata"]["annotations"][UPGRADE_ANNOTATION] = json!("x".repeat(8193))
                }
                _ => {
                    let mut proof: serde_json::Value = serde_json::from_str(
                        member["metadata"]["annotations"][UPGRADE_ANNOTATION]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                    match change {
                        "origin" => proof["origin_sha256"] = json!("f".repeat(64)),
                        "generation" => proof["generation"] = json!(3),
                        "image" => proof["image"] = json!(image('c')),
                        "request" => proof["request_sha256"] = json!("f".repeat(64)),
                        "unknown" => proof["unknown"] = json!(true),
                        _ => unreachable!(),
                    }
                    member["metadata"]["annotations"][UPGRADE_ANNOTATION] =
                        json!(proof.to_string());
                }
            }
        }
        assert!(
            fixture
                .fixture
                .client
                .prove_native_home(&fixture.home)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            client(&fixture.fixture, true)
                .validate_request(&fixture.handle, &next)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            upgrade(&fixture, &fixture.previous, &next).await.is_err(),
            "{change}"
        );
        fixture.no_writes();
    }
}

#[tokio::test]
async fn native_upgrade_waits_for_old_pod_absence_before_any_write() {
    let fixture = Resume::new().await;
    let next = target(&fixture.home, 2, &image('b'));
    let path = "/api/v1/namespaces/cma-test/pods/old-runtime";
    fixture.fixture.state.lock().unwrap().rows.insert(path.into(), json!({"apiVersion":"v1","kind":"Pod",
        "metadata":{"name":"old-runtime","namespace":"cma-test","uid":"old-pod",
            "labels":{crate::agent_sandbox::pods::TRACKING_LABEL:crate::agent_sandbox::pods::name_hash("warm-member")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]}}));
    assert!(tokio::time::timeout(
        Duration::from_millis(100),
        upgrade(&fixture, &fixture.previous, &next)
    )
    .await
    .is_err());
    fixture.no_writes();
    fixture.fixture.state.lock().unwrap().rows.remove(path);
    upgrade(&fixture, &fixture.previous, &next).await.unwrap();
}

#[tokio::test]
async fn native_upgrade_fences_final_cas_against_new_generation_and_policy_changes() {
    for change in [
        "uid",
        "version",
        "origin",
        "generation",
        "mode",
        "proof",
        "policy",
    ] {
        let fixture = Resume::new().await;
        let next = target(&fixture.home, 2, &image('b'));
        let rows = fixture.fixture.state.lock().unwrap().rows.clone();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reads = count.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(
                request.method(),
                http::Method::GET,
                "stale final authority must never write"
            );
            let path = request.uri().path();
            let mut body = if path == "/api/v1/namespaces/cma-test/pods" {
                json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":[]})
            } else {
                rows[path].clone()
            };
            if path == MEMBER && reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
                match change {
                    "uid" => body["metadata"]["uid"] = json!("replacement"),
                    "version" => body["metadata"]["resourceVersion"] = json!(""),
                    "origin" => {
                        body["metadata"]["annotations"][NATIVE_HOME_ANNOTATION] = json!("{}")
                    }
                    "generation" => {
                        body["metadata"]["annotations"]["cma/runtime-generation"] = json!("3")
                    }
                    "mode" => body["spec"]["operatingMode"] = json!("Running"),
                    "proof" => body["metadata"]["annotations"][UPGRADE_ANNOTATION] = json!("{}"),
                    "policy" => {
                        body["spec"]["podTemplate"]["spec"]["serviceAccountName"] = json!("foreign")
                    }
                    _ => unreachable!(),
                }
            }
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(200)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let runtime = AgentSandboxClient::new(
            kube::Client::new(service, "cma-test"),
            fixture.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        assert!(
            runtime
                .upgrade_with_home(&fixture.handle, &fixture.previous, &next, &fixture.home)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}

#[tokio::test]
async fn native_upgrade_bootstrap_proves_exact_upgraded_request_before_delivering_authority() {
    let fixture = Resume::new().await;
    let next = target(&fixture.home, 2, &image('b'));
    upgrade(&fixture, &fixture.previous, &next).await.unwrap();
    {
        let mut state = fixture.fixture.state.lock().unwrap();
        state.rows.insert("/api/v1/namespaces/cma-test/pods/warm-member".into(), json!({"apiVersion":"v1","kind":"Pod",
            "metadata":{"name":"warm-member","namespace":"cma-test","uid":"upgraded-pod",
                "labels":{"cma/control-plane":"cma-test",crate::agent_sandbox::pods::TRACKING_LABEL:crate::agent_sandbox::pods::name_hash("warm-member")},
                "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]},
            "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}]}}));
        let member = state.rows.get_mut(MEMBER).unwrap();
        member["status"]["conditions"][0]["observedGeneration"] =
            member["metadata"]["generation"].clone();
    }
    let runtime = client(&fixture.fixture, true);
    let pod = runtime.pod_binding(&fixture.handle).await.unwrap();
    assert!(runtime
        .assign_pool_runtime(&fixture.handle, &pod, &fixture.previous)
        .await
        .is_err());
    assert!(fixture.fixture.state.lock().unwrap().assignments.is_empty());
    runtime
        .assign_pool_runtime(&fixture.handle, &pod, &next)
        .await
        .unwrap()
        .unwrap();
    {
        let state = fixture.fixture.state.lock().unwrap();
        assert_eq!(state.assignments.len(), 1);
        assert_eq!(state.assignments[0].pod_uid, "upgraded-pod");
        assert_eq!(
            state.assignments[0].environment["CHRONO_RUNTIME_GENERATION"],
            "2"
        );
        assert_eq!(
            state.assignments[0].environment["CHRONO_RUNTIME_HOME_IDENTITY"],
            fixture.home.identity().unwrap()
        );
    }
    // The original native provisioning path cannot use an upgrade as a new admission.
    assert!(runtime
        .provision_pool_runtime_with_home(&next, &fixture.home)
        .await
        .is_err());
}

#[tokio::test]
async fn native_upgrade_supports_full_home_layout_without_changing_mounts_or_vct() {
    let fixture = Fixture::new().await;
    let mut full = spec();
    full.home.mode = crate::HomeVolumeMode::NativeVctFullHomeV2;
    full.image.labels.insert(
        crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
        "2".into(),
    );
    let claim = assigned_spec(&fixture, &full).await;
    let home = adopted(&fixture, &claim).await;
    let previous = request(&home, 1);
    let runtime = client(&fixture, false);
    let handle = runtime
        .admit_pool_runtime(&claim, &home, &previous)
        .await
        .unwrap();
    runtime.suspend(&handle, &previous).await.unwrap();
    let before = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    let next = target(&home, 2, &image('b'));
    runtime
        .upgrade_with_home(&handle, &previous, &next, &home)
        .await
        .unwrap();
    runtime
        .validate_request(&handle, &next)
        .await
        .unwrap()
        .unwrap();
    let after = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    assert_eq!(
        before["volumeClaimTemplates"],
        after["volumeClaimTemplates"]
    );
    assert_eq!(
        before["podTemplate"]["spec"]["containers"][0]["volumeMounts"],
        after["podTemplate"]["spec"]["containers"][0]["volumeMounts"]
    );
    fixture.client.prove_native_home(&home).await.unwrap();
}

#[path = "native_upgrade_hosted_tests.rs"]
mod hosted_tests;

#[tokio::test]
async fn native_upgrade_revalidates_claim_and_pvc_after_pod_removal() {
    for replace_claim in [true, false] {
        let fixture = Resume::new().await;
        let next = target(&fixture.home, 2, &image('b'));
        let rows = fixture.fixture.state.lock().unwrap().rows.clone();
        let target_path = if replace_claim {
            fixture.claim_path.clone()
        } else {
            VOLUME.into()
        };
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reads = count.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET);
            let path = request.uri().path();
            let mut body = if path == "/api/v1/namespaces/cma-test/pods" {
                json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},"items":[]})
            } else {
                rows[path].clone()
            };
            if path == target_path && reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                body["metadata"]["uid"] = json!("replacement-after-drain");
            }
            async move {
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(200)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        let runtime = AgentSandboxClient::new(
            kube::Client::new(service, "cma-test"),
            fixture.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        assert!(runtime
            .upgrade_with_home(&fixture.handle, &fixture.previous, &next, &fixture.home)
            .await
            .is_err());
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn native_upgrade_requires_suspension_and_complete_pod_inventory() {
    for running in [true, false] {
        let fixture = Resume::new().await;
        let next = target(&fixture.home, 2, &image('b'));
        if running {
            fixture
                .fixture
                .state
                .lock()
                .unwrap()
                .rows
                .get_mut(MEMBER)
                .unwrap()["spec"]["operatingMode"] = json!("Running");
        }
        let (reads, runtime) = fixture.reads(BTreeMap::from([("/api/v1/namespaces/cma-test/pods".into(), (200, json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1","continue":"more-pods"},"items":[]})))]), BTreeMap::new());
        assert!(runtime
            .upgrade_with_home(&fixture.handle, &fixture.previous, &next, &fixture.home)
            .await
            .is_err());
        assert!(!reads.calls.lock().unwrap().is_empty());
    }
}

#[path = "native_retired_policy_tests.rs"]
mod retired_policy_tests;
