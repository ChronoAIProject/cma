use super::*;
use crate::agent_sandbox::pool::{
    fixture::prefix,
    read_fixture::{failure, Gate, Reads},
};
use std::{collections::BTreeMap, time::Duration};

#[tokio::test]
async fn native_bootstrap_policy_and_graph_overlap_without_dispatch_on_either_failure() {
    let bound = bound(true).await;
    let pod = client(&bound.fixture, false)
        .pod_binding(&bound.runtime)
        .await
        .unwrap();
    let template = format!(
        "{}/sandboxtemplates/{}",
        prefix(),
        bound.claim.request.pool.template_name
    );
    let pool = format!(
        "{}/sandboxwarmpools/{}",
        prefix(),
        bound.claim.request.pool.name
    );
    let claim = format!("{}/sandboxclaims/{}", prefix(), bound.claim.name);
    let pods = "/api/v1/namespaces/cma-test/pods";
    let paths = [
        template.clone(),
        pool,
        claim.clone(),
        VOLUME.into(),
        pods.into(),
    ];
    let pod_list = json!({"apiVersion":"v1","kind":"PodList","metadata":{"resourceVersion":"1"},
        "items":[bound.fixture.state.lock().unwrap().rows[POD].clone()]});
    // Both completion orders and either failed branch must retain policy-first
    // errors. A valid graph cannot dispatch while policy remains pending.
    for (policy_first, policy_fails, graph_fails) in [
        (false, true, false),
        (true, false, true),
        (false, true, true),
        (true, true, true),
    ] {
        let gates: BTreeMap<_, _> = paths.iter().map(|p| (p.clone(), Gate::new())).collect();
        let mut overrides = BTreeMap::from([(pods.into(), (200, pod_list.clone()))]);
        if policy_fails {
            overrides.insert(template.clone(), failure(429));
        }
        if graph_fails {
            overrides.insert(claim.clone(), failure(403));
        }
        let reads = Reads::new(&bound.fixture, overrides, gates.clone());
        let client = crate::agent_sandbox::AgentSandboxClient::new(
            reads.client.clone(),
            bound.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        let runtime = bound.runtime.clone();
        let expected = pod.clone();
        let request = bound.request.clone();
        let mut task = tokio::spawn(async move {
            client
                .assign_pool_runtime(&runtime, &expected, &request)
                .await
        });
        for gate in gates.values() {
            gate.wait().await;
        }
        for (i, path) in paths.iter().enumerate() {
            if (i < 2) == policy_first {
                gates[path].respond();
            }
        }
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 1);
        for (i, path) in paths.iter().enumerate() {
            if (i < 2) != policy_first {
                gates[path].respond();
            }
        }
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            matches!(error, AdapterError::AtCapacity(_)),
            policy_fails,
            "{error:?}"
        );
        assert_eq!(
            reads.calls.lock().unwrap().len(),
            7,
            "initial member plus two policy and four graph reads, no bootstrap"
        );
        assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 1);
    }
}

#[tokio::test]
async fn native_bootstrap_uses_one_authority_proof_before_and_after_dispatch() {
    for mode in [
        HomeVolumeMode::NativeVctEmptyV1,
        HomeVolumeMode::NativeVctFullHomeV2,
    ] {
        for explicit in [false, true] {
            let bound = bound_with_mode(explicit, mode).await;
            let client = client(&bound.fixture, false);
            let pod = client.pod_binding(&bound.runtime).await.unwrap();
            for _ in 0..2 {
                bound.fixture.state.lock().unwrap().calls.clear();
                client
                    .assign_pool_runtime(&bound.runtime, &pod, &bound.request)
                    .await
                    .unwrap()
                    .unwrap();
                let state = bound.fixture.state.lock().unwrap();
                assert!(state
                    .calls
                    .iter()
                    .all(|(method, _)| method.starts_with("GET ")));
                // Explicit policy separately verifies both Pool and Template.
                assert_eq!(state.calls.len(), 9 + 2 * usize::from(explicit));
                for resource in ["/sandboxclaims/", "/persistentvolumeclaims/", "/pods"] {
                    assert_eq!(
                        state
                            .calls
                            .iter()
                            .filter(|(call, _)| call.contains(resource))
                            .count(),
                        2,
                        "{resource}"
                    );
                }
                assert_eq!(
                    state
                        .calls
                        .iter()
                        .filter(|(call, _)| call.contains("/sandboxes/"))
                        .count(),
                    3
                );
            }
            assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 3);
        }
    }
}

const CHANGES: [&str; 9] = [
    "claim_uid",
    "claim_origin",
    "member_uid",
    "member_spec",
    "member_annotation",
    "pvc_uid",
    "pvc_layout",
    "pod_uid",
    "container",
];

fn mutation(bound: &Bound, change: &str) -> (String, String, serde_json::Value) {
    let (path, pointer, value) = match change {
        "claim_uid" => (
            format!(
                "/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxclaims/{}",
                bound.claim.name
            ),
            "/metadata/uid",
            json!("replacement"),
        ),
        "claim_origin" => (
            format!(
                "/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/cma-test/sandboxclaims/{}",
                bound.claim.name
            ),
            "/metadata/annotations/cma~1native-home-origin",
            json!("{}"),
        ),
        "member_uid" => (MEMBER.into(), "/metadata/uid", json!("replacement")),
        "member_spec" => (
            MEMBER.into(),
            "/spec/podTemplate/spec/containers/0/image",
            json!("replacement"),
        ),
        "member_annotation" => (
            MEMBER.into(),
            "/metadata/annotations/cma~1runtime-generation",
            json!("42"),
        ),
        "pvc_uid" => (VOLUME.into(), "/metadata/uid", json!("replacement")),
        "pvc_layout" => (
            VOLUME.into(),
            "/metadata/annotations/cma~1home-layout",
            json!("another-layout"),
        ),
        "pod_uid" => (POD.into(), "/metadata/uid", json!("replacement")),
        "container" => (
            POD.into(),
            "/status/containerStatuses/0/containerID",
            json!("containerd://replacement"),
        ),
        _ => unreachable!(),
    };
    (path, pointer.into(), value)
}

#[tokio::test]
async fn native_bootstrap_refuses_changed_live_authority_before_sending_credentials() {
    for change in CHANGES {
        let bound = bound(false).await;
        let client = client(&bound.fixture, false);
        let pod = client.pod_binding(&bound.runtime).await.unwrap();
        let (path, pointer, value) = mutation(&bound, change);
        *bound
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .get_mut(&path)
            .unwrap()
            .pointer_mut(&pointer)
            .unwrap() = value;
        assert!(
            client
                .assign_pool_runtime(&bound.runtime, &pod, &bound.request)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            1,
            "{change}"
        );
    }
}

#[tokio::test]
async fn native_bootstrap_rechecks_all_authority_after_exec_before_binding_or_caching() {
    for change in CHANGES {
        let bound = bound(false).await;
        let mutation = mutation(&bound, change);
        let (path, pointer, _) = &mutation;
        let old = bound.fixture.state.lock().unwrap().rows[path]
            .pointer(pointer)
            .unwrap()
            .clone();
        bound
            .fixture
            .state
            .lock()
            .unwrap()
            .exec_mutations
            .push(mutation.clone());
        let restarted =
            AgentSandboxBackend::new(client(&bound.fixture, false), bound.transport.clone());
        assert!(
            restarted
                .adopt(&bound.runtime, &bound.request)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            2,
            "{change}"
        );
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 1, "{change}");
        *bound
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .get_mut(path)
            .unwrap()
            .pointer_mut(pointer)
            .unwrap() = old;
        restarted
            .adopt(&bound.runtime, &bound.request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            3,
            "{change}"
        );
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 2, "{change}");
    }
}
