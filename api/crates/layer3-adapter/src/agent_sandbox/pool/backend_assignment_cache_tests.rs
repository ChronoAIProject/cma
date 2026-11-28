use super::*;
use crate::agent_sandbox::NativePoolClaim;
use crate::provider::ProviderSandboxRequest;
use serde_json::json;

#[path = "nonexecuting_tests.rs"]
mod nonexecuting;

#[path = "backend_provision_home_tests.rs"]
mod provision_home_tests;

#[path = "backend_adoption_home_tests.rs"]
mod adoption_home_tests;

#[path = "backend_relay_retry_tests.rs"]
mod relay_retry_tests;

const POD: &str = "/api/v1/namespaces/cma-test/pods/warm-member";

struct Bound {
    fixture: Fixture,
    backend: AgentSandboxBackend,
    transport: Arc<Transport>,
    request: ProviderSandboxRequest,
    runtime: RuntimeHandle,
    claim: NativePoolClaim,
}

async fn bound(explicit_policy: bool) -> Bound {
    bound_with_mode(explicit_policy, HomeVolumeMode::NativeVctEmptyV1).await
}

async fn bound_with_mode(explicit_policy: bool, mode: HomeVolumeMode) -> Bound {
    bound_with_image(explicit_policy, mode, None).await
}

async fn bound_with_image(
    explicit_policy: bool,
    mode: HomeVolumeMode,
    image: Option<&str>,
) -> Bound {
    bound_config(explicit_policy, mode, image, false, false).await
}

async fn bound_config(
    explicit_policy: bool,
    mode: HomeVolumeMode,
    image: Option<&str>,
    startup_breaker: bool,
    with_home: bool,
) -> Bound {
    let mut fixture = Fixture::new().await;
    fixture.client.config.runtime_startup_breaker_enabled = startup_breaker;
    if explicit_policy {
        fixture.client.config.template_network_policy =
            Some(super::super::network_policy_tests::policy("10.254.0.0/16"));
    }
    let mut spec = super::super::tests::spec();
    spec.home.mode = mode;
    if let Some(image) = image {
        spec.image.uri = image.into();
    }
    spec.image.labels.insert(
        crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
        "2".into(),
    );
    if spec.home.is_native_vct() {
        spec.home.template_name.clear();
        spec.home.template_generation.clear();
    }
    let claim = super::super::home_tests::assigned_spec(&fixture, &spec).await;
    let home = adopted(&fixture, &claim).await;
    let request = super::super::runtime_tests::request_with_spec(&home, 1, &spec);
    fixture.state.lock().unwrap().rows.insert(POD.into(), json!({
        "apiVersion":"v1","kind":"Pod",
        "metadata":{"name":"warm-member","namespace":"cma-test","uid":"original-pod",
            "labels":{"cma/control-plane":"cma-test",super::super::super::pods::TRACKING_LABEL:super::super::super::pods::name_hash("warm-member")},
            "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]},
        "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}],
            "containerStatuses":[{"name":"runtime","image":"runtime","imageID":"image-id","containerID":format!("containerd://{}", "a".repeat(64)),
                "ready":true,"restartCount":0,"state":{"running":{}}}]}
    }));
    let transport = Arc::new(Transport {
        state: fixture.state.clone(),
        binds: Mutex::new(vec![]),
    });
    let backend = AgentSandboxBackend::new(client(&fixture, false), transport.clone());
    let runtime = if with_home {
        backend
            .provision_with_home(request.clone(), &home)
            .await
            .unwrap()
    } else {
        backend.provision(request.clone()).await.unwrap()
    };
    Bound {
        fixture,
        backend,
        transport,
        request,
        runtime,
        claim,
    }
}

#[tokio::test]
async fn exact_adoption_reuses_acknowledgement_with_fresh_claim_home_and_pod_proofs() {
    let bound = bound(false).await;
    bound.fixture.state.lock().unwrap().calls.clear();
    bound
        .backend
        .adopt(&bound.runtime, &bound.request)
        .await
        .unwrap()
        .unwrap();
    let state = bound.fixture.state.lock().unwrap();
    assert_eq!(state.assignments.len(), 1);
    assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
    for resource in [
        "/sandboxclaims/",
        "/sandboxes/",
        "/persistentvolumeclaims/",
        "/pods",
    ] {
        assert!(
            state.calls.iter().any(|(call, _)| call.contains(resource)),
            "missing fresh {resource} proof"
        );
    }
    assert!(
        state.calls.len() <= 14,
        "cached adoption keeps pre-bind authority and fresh post-bind exact proof"
    );
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cached_adoption_refuses_replaced_claim_pvc_and_changed_request_or_network_policy() {
    for change in ["claim", "volume", "request", "policy"] {
        let mut bound = bound(change == "policy").await;
        match change {
            "claim" => bound
                .fixture
                .edit("sandboxclaims", &bound.claim.name, |claim| {
                    claim["metadata"]["uid"] = json!("replacement");
                }),
            "volume" => {
                bound
                    .fixture
                    .state
                    .lock()
                    .unwrap()
                    .rows
                    .get_mut(VOLUME)
                    .unwrap()["metadata"]["uid"] = json!("replacement")
            }
            "request" => {
                bound.request.env.insert("CHANGED".into(), "value".into());
            }
            "policy" => bound.fixture.edit(
                "sandboxtemplates",
                &bound.claim.request.pool.template_name,
                |template| {
                    template["spec"]["networkPolicy"]["egress"] = json!([{}]);
                },
            ),
            _ => unreachable!(),
        }
        assert!(
            bound
                .backend
                .adopt(&bound.runtime, &bound.request)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            1,
            "{change}"
        );
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 1, "{change}");
    }
}

#[tokio::test]
async fn container_restart_pod_replacement_and_backend_restart_require_new_acknowledgements() {
    let bound = bound(false).await;
    for (pod_uid, container, restarts) in [
        ("original-pod", "containerd://restarted", 1),
        ("replacement-pod", "containerd://replacement", 0),
    ] {
        let before = bound.fixture.state.lock().unwrap().assignments.len();
        {
            let mut state = bound.fixture.state.lock().unwrap();
            let pod = state.rows.get_mut(POD).unwrap();
            pod["metadata"]["uid"] = json!(pod_uid);
            pod["status"]["containerStatuses"][0]["containerID"] = json!(container);
            pod["status"]["containerStatuses"][0]["restartCount"] = json!(restarts);
        }
        bound
            .backend
            .adopt(&bound.runtime, &bound.request)
            .await
            .unwrap()
            .unwrap();
        bound
            .backend
            .adopt(&bound.runtime, &bound.request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            before + 1
        );
    }
    let restarted =
        AgentSandboxBackend::new(client(&bound.fixture, false), bound.transport.clone());
    restarted
        .adopt(&bound.runtime, &bound.request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 4);
}

#[tokio::test]
async fn failed_assignment_and_missing_container_incarnation_never_enter_cache() {
    let bound = bound(false).await;
    {
        let mut state = bound.fixture.state.lock().unwrap();
        state.rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0]["restartCount"] =
            json!(1);
        state.fail_exec = true;
    }
    assert!(bound
        .backend
        .adopt(&bound.runtime, &bound.request)
        .await
        .is_err());
    bound.fixture.state.lock().unwrap().fail_exec = false;
    bound
        .backend
        .adopt(&bound.runtime, &bound.request)
        .await
        .unwrap()
        .unwrap();
    bound
        .backend
        .adopt(&bound.runtime, &bound.request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 3);
    bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(POD)
        .unwrap()["status"]["containerStatuses"][0]
        .as_object_mut()
        .unwrap()
        .remove("containerID");
    bound
        .backend
        .adopt(&bound.runtime, &bound.request)
        .await
        .unwrap()
        .unwrap();
    bound
        .backend
        .adopt(&bound.runtime, &bound.request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 5);
}

#[path = "backend_native_proof_tests.rs"]
mod native_proof_tests;

#[tokio::test]
async fn contextual_native_resume_waits_for_new_pod_then_assigns_original_home_and_generation() {
    let bound = bound(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    let next = request(&home, 2);
    let mut pod = bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .remove(POD)
        .unwrap();
    bound
        .backend
        .suspend(&bound.runtime, &bound.request)
        .await
        .unwrap();
    // Model a completed update whose response was lost before backend binding.
    client(&bound.fixture, false)
        .resume_with_home(&bound.runtime, &bound.request, &next, &home)
        .await
        .unwrap();
    {
        let mut state = bound.fixture.state.lock().unwrap();
        pod["metadata"]["uid"] = json!("resumed-pod");
        pod["status"]["containerStatuses"][0]["containerID"] = json!("containerd://resumed");
        state.rows.insert(POD.into(), pod);
        let member = state.rows.get_mut(MEMBER).unwrap();
        member["status"]["conditions"][0]["observedGeneration"] =
            member["metadata"]["generation"].clone();
    }
    bound
        .backend
        .resume_with_home(&bound.runtime, &bound.request, &next, &home)
        .await
        .unwrap();
    {
        let mut state = bound.fixture.state.lock().unwrap();
        assert_eq!(state.assignments.len(), 2);
        assert_eq!(state.assignments[1].pod_uid, "resumed-pod");
        assert_eq!(
            state.assignments[1].environment["CHRONO_RUNTIME_GENERATION"],
            "2"
        );
        assert_eq!(
            state.assignments[1].environment["CHRONO_RUNTIME_HOME_IDENTITY"],
            home.identity().unwrap()
        );
        state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement");
    }
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 2);
    assert!(bound
        .backend
        .resume_with_home(&bound.runtime, &bound.request, &next, &home)
        .await
        .is_err());
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 2);
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 2);
}

#[path = "backend_bootstrap_proof_tests.rs"]
mod bootstrap_proof_tests;

#[path = "startup_breaker_tests.rs"]
mod startup_breaker_tests;
