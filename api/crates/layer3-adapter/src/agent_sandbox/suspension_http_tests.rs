use super::*;

fn retained(
    generation: u64,
    name: Option<&str>,
) -> (crate::provider::ProviderSandboxRequest, Value, Value) {
    let mut request = super::super::tests::request();
    request
        .metadata
        .insert("cma/runtime-generation".into(), generation.to_string());
    request
        .env
        .insert("CHRONO_RUNTIME_GENERATION".into(), generation.to_string());
    let allocation = HomeVolumeRequest {
        scope: None,
        volume_id: "home-one".into(),
        sandbox_id: "logical-sandbox".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "standard".into(),
            capacity_gib: 10,
            template_name: "seed".into(),
            template_generation: "one".into(),
        },
    };
    let attachment = crate::HomeVolumeAttachment {
        receipt: crate::HomeVolumeReceipt {
            native_authority: None,
            volume_id: allocation.volume_id.clone(),
            request_sha256: allocation.computed_sha256(),
            storage_reference: "cma-home-test".into(),
            storage_generation: "original-pvc-uid".into(),
        },
        request: allocation.clone(),
    };
    request
        .metadata
        .insert("cma/home-identity".into(), attachment.identity().unwrap());
    let config = super::super::tests::config();
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap())).unwrap(),
        config.clone(),
        "fixture",
    )
    .unwrap();
    let name = name
        .map(str::to_owned)
        .unwrap_or_else(|| model::name(&request).unwrap());
    let mut object = serde_json::to_value(
        model::desired_at(&config, &request, &client.control_token(&name), &name).unwrap(),
    )
    .unwrap();
    object["metadata"]["uid"] = json!("original-crd");
    object["metadata"]["resourceVersion"] = json!("7");
    object["metadata"]["generation"] = json!(1);
    let pvc = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{
        "name":"cma-home-test","namespace":"cma-test","uid":"original-pvc-uid",
        "labels":{"cma/home-managed-by":"cma-test"},"annotations":{
            "cma/home-allocation":serde_json::to_string(&allocation).unwrap(),"cma/home-request-sha256":allocation.computed_sha256()}}});
    (request, object, pvc)
}

fn runtime(object: &Value) -> RuntimeHandle {
    RuntimeHandle::new(format!(
        "agent-sandbox:{}:original-crd",
        object["metadata"]["name"].as_str().unwrap()
    ))
}

#[tokio::test]
async fn suspension_retains_crd_and_pvc_and_waits_for_terminating_pods() {
    let (request, object, pvc) = retained(2, None);
    let handle = runtime(&object);
    let mut suspended = object.clone();
    suspended["spec"]["operatingMode"] = json!("Suspended");
    let mut pods = list("PodList");
    pods["items"] = json!([{"metadata":{"name":"terminating","deletionTimestamp":"2026-10-13T00:00:00Z","ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":object["metadata"]["name"],"uid":"original-crd"}]}}]);
    let fixture = Fixture::start(vec![
        (200, pvc),
        (200, object),
        (200, suspended.clone()),
        (200, suspended.clone()),
        (200, pods),
        (200, suspended),
        (200, list("PodList")),
    ])
    .await;
    fixture.client.suspend(&handle, &request).await.unwrap();
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 7);
    assert_eq!(calls[2].1["metadata"]["uid"], "original-crd");
    assert_eq!(calls[2].1["metadata"]["resourceVersion"], "7");
    assert_eq!(calls[2].1["spec"], json!({"operatingMode":"Suspended"}));
    assert!(calls[4]
        .0
        .contains("labelSelector=agents.x-k8s.io%2Fsandbox-name-hash%3D"));
    assert!(!calls[4].0.contains("cma%2Fruntime-name"));
    assert!(calls
        .iter()
        .all(|(line, _)| !line.starts_with("DELETE ") && !line.starts_with("POST ")));
}

#[tokio::test]
async fn resume_reuses_original_name_uid_and_pvc_with_a_new_generation() {
    let (previous, mut old, pvc) = retained(2, None);
    let handle = runtime(&old);
    let name = old["metadata"]["name"].as_str().unwrap().to_owned();
    let (next, mut resumed, _) = retained(3, Some(&name));
    old["spec"]["operatingMode"] = json!("Suspended");
    resumed["metadata"]["resourceVersion"] = json!("8");
    let fixture = Fixture::start(vec![
        (200, pvc.clone()),
        (200, pvc.clone()),
        (200, old.clone()),
        (200, old.clone()),
        (200, list("PodList")),
        (200, old),
        (0, Value::Null),
        // Lost acknowledgement / restarted controller replay observes the same next identity.
        (200, pvc.clone()),
        (200, pvc),
        (200, resumed),
    ])
    .await;
    assert!(matches!(
        fixture.client.resume(&handle, &previous, &next).await,
        Err(AdapterError::Unavailable(_))
    ));
    fixture
        .client
        .resume(&handle, &previous, &next)
        .await
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    let patches: Vec<_> = calls
        .iter()
        .filter(|(line, _)| line.starts_with("PATCH "))
        .collect();
    assert_eq!(patches.len(), 1);
    let (line, patch) = patches[0];
    assert!(line.contains(&name));
    assert_eq!(patch["metadata"]["uid"], "original-crd");
    assert_eq!(
        patch["metadata"]["annotations"]["cma/runtime-generation"],
        "3"
    );
    assert_eq!(patch["spec"]["operatingMode"], "Running");
    assert_eq!(
        patch["spec"]["podTemplate"]["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "cma-home-test"
    );
    assert!(calls
        .iter()
        .all(|(line, _)| !line.starts_with("POST ") && !line.starts_with("DELETE ")));
}

#[tokio::test]
async fn suspended_observation_requires_complete_pod_inventory_and_mutations_require_a_version() {
    let (request, mut object, pvc) = retained(2, None);
    let handle = runtime(&object);
    object["spec"]["operatingMode"] = json!("Suspended");
    let mut partial = list("PodList");
    partial["metadata"]["continue"] = json!("next-page");
    let fixture = Fixture::start(vec![
        (200, pvc.clone()),
        (200, object.clone()),
        (200, partial),
    ])
    .await;
    assert!(matches!(
        fixture.client.verify_suspended(&handle, &request).await,
        Err(AdapterError::Unavailable(_))
    ));
    assert_eq!(fixture.calls.lock().unwrap().len(), 3);
    object["spec"]["operatingMode"] = json!("Running");
    object["metadata"]
        .as_object_mut()
        .unwrap()
        .remove("resourceVersion");
    let fixture = Fixture::start(vec![(200, pvc), (200, object)]).await;
    assert!(matches!(
        fixture.client.suspend(&handle, &request).await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(fixture.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn changed_uid_or_image_cannot_be_resumed_or_suspended_by_stale_authority() {
    let (request, mut object, pvc) = retained(2, None);
    let handle = runtime(&object);
    object["metadata"]["uid"] = json!("replacement-crd");
    let fixture = Fixture::start(vec![(200, pvc), (200, object)]).await;
    assert!(matches!(
        fixture.client.suspend(&handle, &request).await,
        Err(AdapterError::Rejected(_))
    ));
    let mut next = request.clone();
    next.metadata
        .insert("cma/runtime-generation".into(), "3".into());
    next.env
        .insert("CHRONO_RUNTIME_GENERATION".into(), "3".into());
    next.image.uri = format!("registry.example/cma@sha256:{}", "b".repeat(64));
    assert!(matches!(
        fixture.client.resume(&handle, &request, &next).await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(fixture.calls.lock().unwrap().len(), 2);
}

fn scoped_retained(
    generation: u64,
    name: Option<&str>,
    reaches: &str,
    model_service: &str,
) -> (crate::provider::ProviderSandboxRequest, Value, Value) {
    let (mut request, original, pvc) = retained(generation, name);
    request
        .env
        .insert("CHRONO_SANDBOX_REACHES".into(), reaches.into());
    request.env.insert(
        "CHRONO_PLATFORM_BASE_URL".into(),
        "https://nyx.example".into(),
    );
    request.env.insert(
        "CHRONO_UPSTREAM_BASE_URL".into(),
        format!("https://nyx.example/api/v1/proxy/s/{model_service}"),
    );
    let config = super::super::tests::config();
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap())).unwrap(),
        config.clone(),
        "fixture",
    )
    .unwrap();
    let name = original["metadata"]["name"].as_str().unwrap();
    let mut object = serde_json::to_value(
        model::desired_at(&config, &request, &client.control_token(name), name).unwrap(),
    )
    .unwrap();
    for field in ["uid", "resourceVersion", "generation"] {
        object["metadata"][field] = original["metadata"][field].clone();
    }
    (request, object, pvc)
}

#[tokio::test]
async fn scope_resume_keeps_uid_version_and_absent_pod_fences_and_replays_exact_new_identity() {
    let (previous, mut old, pvc) = scoped_retained(2, None, "chrono-llm", "chrono-llm");
    let handle = runtime(&old);
    let name = old["metadata"]["name"].as_str().unwrap().to_owned();
    let (next, mut resumed, _) = scoped_retained(
        3,
        Some(&name),
        "chrono-llm,chrono-llm-public,api-github",
        "chrono-llm-public",
    );
    old["spec"]["operatingMode"] = json!("Suspended");
    resumed["metadata"]["resourceVersion"] = json!("8");
    let fixture = Fixture::start(vec![
        (200, pvc.clone()),
        (200, pvc.clone()),
        (200, old.clone()),
        (200, old.clone()),
        (200, list("PodList")),
        (200, old),
        (0, Value::Null),
        (200, pvc.clone()),
        (200, pvc),
        (200, resumed.clone()),
    ])
    .await;
    assert!(matches!(
        fixture.client.resume(&handle, &previous, &next).await,
        Err(AdapterError::Unavailable(_))
    ));
    fixture
        .client
        .resume(&handle, &previous, &next)
        .await
        .unwrap();
    let calls = fixture.calls.lock().unwrap();
    let patches: Vec<_> = calls
        .iter()
        .filter(|(line, _)| line.starts_with("PATCH "))
        .collect();
    assert_eq!(patches.len(), 1);
    let patch = &patches[0].1;
    assert_eq!(patch["metadata"]["uid"], "original-crd");
    assert_eq!(patch["metadata"]["resourceVersion"], "7");
    assert_eq!(patch["spec"], resumed["spec"]);
    assert_eq!(
        patch["metadata"]["annotations"],
        resumed["metadata"]["annotations"]
    );
    assert!(calls[4].0.contains("/pods?"));
    assert!(calls
        .iter()
        .all(|(line, _)| !line.starts_with("POST ") && !line.starts_with("DELETE ")));
}

#[tokio::test]
async fn scope_resume_refuses_unrelated_runtime_changes_before_kubernetes_io() {
    let (previous, old, _) = scoped_retained(2, None, "chrono-llm", "chrono-llm");
    let handle = runtime(&old);
    let (next, _, _) = scoped_retained(
        3,
        Some(old["metadata"]["name"].as_str().unwrap()),
        "chrono-llm,chrono-llm-public",
        "chrono-llm-public",
    );
    let fixture = Fixture::start(vec![]).await;
    for field in [
        "origin",
        "route",
        "query",
        "platform",
        "tool",
        "image",
        "mount",
        "home",
        "owner",
        "generation",
        "removed-scope",
    ] {
        let mut changed = next.clone();
        match field {
            "origin" => {
                changed.env.insert(
                    "CHRONO_UPSTREAM_BASE_URL".into(),
                    "https://other.example/api/v1/proxy/s/chrono-llm-public".into(),
                );
            }
            "route" => {
                changed.env.insert(
                    "CHRONO_UPSTREAM_BASE_URL".into(),
                    "https://nyx.example/arbitrary".into(),
                );
            }
            "query" => {
                changed.env.insert(
                    "CHRONO_UPSTREAM_BASE_URL".into(),
                    "https://nyx.example/api/v1/proxy/s/chrono-llm-public?changed=1".into(),
                );
            }
            "platform" => {
                changed.env.insert(
                    "CHRONO_PLATFORM_BASE_URL".into(),
                    "https://other.example".into(),
                );
            }
            "tool" => {
                changed.env.insert(
                    "CHRONO_TOOL_PROXY_URL".into(),
                    "https://other.example/mcp".into(),
                );
            }
            "image" => {
                changed.image.uri = format!("registry.example/cma@sha256:{}", "b".repeat(64))
            }
            "mount" => changed.volumes[0].mount_path = "/other".into(),
            "home" => {
                changed
                    .metadata
                    .insert("cma/home-identity".into(), "replacement".into());
            }
            "owner" => {
                changed
                    .metadata
                    .insert("cma/sandbox".into(), "other".into());
            }
            "generation" => {
                changed
                    .env
                    .insert("CHRONO_RUNTIME_GENERATION".into(), "4".into());
            }
            "removed-scope" => {
                changed.env.remove("CHRONO_SANDBOX_REACHES");
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                fixture.client.resume(&handle, &previous, &changed).await,
                Err(AdapterError::Rejected(_))
            ),
            "{field}"
        );
    }
    assert!(fixture.calls.lock().unwrap().is_empty());
}
