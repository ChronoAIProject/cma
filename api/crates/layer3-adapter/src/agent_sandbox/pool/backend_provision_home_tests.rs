use super::*;
use crate::agent_sandbox::{
    pool::read_fixture::{Gate, Reads},
    AgentSandboxClient,
};
use std::collections::BTreeMap;

struct NativeRoute;
#[async_trait::async_trait]
impl crate::hosted::RuntimeRoutingStore for NativeRoute {
    async fn select(&self, sandbox: &str, generation: u64, _: bool) -> Result<bool> {
        assert_eq!((sandbox, generation), ("sbx_one", 1));
        Ok(true)
    }
}

fn counts(bound: &Bound) -> BTreeMap<String, usize> {
    let state = bound.fixture.state.lock().unwrap();
    let mut counts = BTreeMap::new();
    for (call, _) in &state.calls {
        assert!(call.starts_with("GET "), "replay must not publish again");
        *counts.entry(call.clone()).or_default() += 1;
    }
    counts
}

#[tokio::test]
async fn persisted_home_provision_removes_only_native_origin_discovery_reads() {
    for mode in [
        HomeVolumeMode::default(),
        HomeVolumeMode::NativeVctEmptyV1,
        HomeVolumeMode::NativeVctFullHomeV2,
    ] {
        for explicit in [false, true] {
            let bound = bound_with_mode(explicit, mode).await;
            let home = adopted(&bound.fixture, &bound.claim).await;
            bound.fixture.state.lock().unwrap().calls.clear();
            let discovered = bound
                .backend
                .provision(bound.request.clone())
                .await
                .unwrap();
            let mut expected = counts(&bound);
            if home.request.policy.is_native_vct() {
                // The original route first discovers PVC -> Sandbox origin,
                // then runs the very same fresh admission/bootstrap proofs.
                *expected.get_mut(&format!("GET {VOLUME}")).unwrap() -= 1;
                *expected.get_mut(&format!("GET {MEMBER}")).unwrap() -= 1;
            }
            bound.fixture.state.lock().unwrap().calls.clear();
            assert_eq!(
                bound
                    .backend
                    .provision_with_home(bound.request.clone(), &home)
                    .await
                    .unwrap(),
                discovered
            );
            assert_eq!(counts(&bound), expected, "{mode:?}, explicit={explicit}");
            assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 3);
            assert_eq!(bound.transport.binds.lock().unwrap().len(), 3);
        }
    }
}

#[tokio::test]
async fn adapter_and_provider_router_preserve_home_and_both_home_verification_boundaries() {
    use crate::{
        agent_sandbox::NativeHomeVolumes,
        codex::{CodexAdapter, CodexSandboxSpec},
        hosted::{HostedDeployment, RuntimeBackends},
    };
    let bound = bound(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    bound.fixture.state.lock().unwrap().calls.clear();
    bound
        .backend
        .provision(bound.request.clone())
        .await
        .unwrap();
    let mut expected = counts(&bound);
    // Adapter still verifies Claim/member/PVC before and after provisioning;
    // only the backend's redundant PVC/member discovery is removed.
    let claim_path = format!(
        "GET {}/sandboxclaims/{}",
        crate::agent_sandbox::pool::fixture::prefix(),
        bound.claim.name
    );
    *expected.get_mut(&claim_path).unwrap() += 2;
    *expected.get_mut(&format!("GET {MEMBER}")).unwrap() += 1;
    *expected.get_mut(&format!("GET {VOLUME}")).unwrap() += 1;
    let homes = Arc::new(
        RoutedHomeVolumes::new(
            None,
            Some((
                Arc::new(
                    NativeHomeVolumes::new(
                        bound.fixture.kubernetes.clone(),
                        bound.fixture.client.config.clone(),
                        "fixture",
                    )
                    .unwrap(),
                ),
                home.request.policy.clone(),
            )),
        )
        .unwrap(),
    );
    let backend = Arc::new(RuntimeBackends {
        agent_sandbox: Some(Arc::new(AgentSandboxBackend::new(
            client(&bound.fixture, false),
            bound.transport.clone(),
        ))),
        opensandbox: None,
        allocate_agent_sandbox: false,
        routing: Arc::new(NativeRoute),
        home_routes: Some(homes.clone()),
    });
    let config = super::super::super::tests::spec();
    let adapter = CodexAdapter::new(
        backend,
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image: config.image.clone(),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .with_home_volumes(homes);
    let spec = SandboxSpec {
        sandbox_id: home.request.sandbox_id.clone(),
        runtime_image: Some(config.image.uri),
        created_by: Some("cma-test".into()),
        home_volume: Some(Box::new(home)),
        capacity: SandboxCapacity {
            cpu_millis: config.cpu_millis,
            memory_mib: config.memory_mib,
        },
        ..Default::default()
    };
    bound.fixture.state.lock().unwrap().calls.clear();
    assert_eq!(
        adapter
            .provision_runtime(&RuntimeProvisionRequest::new(1, spec))
            .await
            .unwrap(),
        bound.runtime
    );
    assert_eq!(counts(&bound), expected);
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 3);
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn persisted_home_provision_starts_fresh_graph_without_serial_origin_discovery() {
    let bound = bound(false).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    let paths = [
        format!(
            "{}/sandboxclaims/{}",
            crate::agent_sandbox::pool::fixture::prefix(),
            bound.claim.name
        ),
        MEMBER.into(),
        VOLUME.into(),
    ];
    let gates: BTreeMap<_, _> = paths
        .iter()
        .map(|path| (path.clone(), Gate::new()))
        .collect();
    let reads = Reads::new(&bound.fixture, BTreeMap::new(), gates.clone());
    let client = AgentSandboxClient::new(
        reads.client.clone(),
        bound.fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    let request = bound.request.clone();
    let task = tokio::spawn(async move {
        client
            .provision_pool_runtime_with_home(&request, &home)
            .await
    });
    for gate in gates.values() {
        gate.wait().await;
    }
    assert_eq!(reads.calls.lock().unwrap().len(), 3);
    for gate in gates.values() {
        gate.respond();
    }
    assert_eq!(task.await.unwrap().unwrap(), bound.runtime);
    assert_eq!(reads.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn persisted_home_provision_rejects_forged_home_and_request_before_assignment() {
    for change in [
        "receipt",
        "missing_origin",
        "scope",
        "claim_uid",
        "pvc_uid",
        "sandbox",
        "home_identity",
        "generation",
        "mount",
        "image",
    ] {
        let bound = bound(false).await;
        let mut home = adopted(&bound.fixture, &bound.claim).await;
        let mut requested = bound.request.clone();
        match change {
            "receipt" => home.receipt.request_sha256 = "forged".into(),
            "missing_origin" => home.receipt.native_authority = None,
            "scope" => {
                home.request.scope = Some(
                    HomeVolumeScope::from_authority(
                        "native-vct-empty-v1",
                        &["foreign", "cma-test", "cma-test"],
                    )
                    .unwrap(),
                );
                home.receipt
                    .native_authority
                    .as_mut()
                    .unwrap()
                    .claim
                    .request
                    .home = home.request.clone();
                home.receipt.request_sha256 = home.request.computed_sha256();
                requested = request(&home, 1);
            }
            "claim_uid" => {
                home.receipt.native_authority.as_mut().unwrap().claim.uid = "forged-claim".into();
                requested = request(&home, 1);
            }
            "pvc_uid" => {
                home.receipt.storage_generation = "forged-volume".into();
                home.receipt.native_authority.as_mut().unwrap().pvc_uid = "forged-volume".into();
                requested = request(&home, 1);
            }
            "sandbox" => {
                requested
                    .metadata
                    .insert("cma/sandbox".into(), "another-sandbox".into());
            }
            "home_identity" => {
                requested
                    .metadata
                    .insert("cma/home-identity".into(), "forged".into());
            }
            "generation" => requested = request(&home, 2),
            "mount" => requested.volumes[0].pvc.claim_name = "another-home".into(),
            "image" => requested.image.uri = "another-image".into(),
            _ => unreachable!(),
        }
        bound.fixture.state.lock().unwrap().calls.clear();
        assert!(
            bound
                .backend
                .provision_with_home(requested, &home)
                .await
                .is_err(),
            "{change}"
        );
        let state = bound.fixture.state.lock().unwrap();
        assert_eq!(state.assignments.len(), 1, "{change}");
        assert!(
            state.calls.iter().all(|(call, _)| call.starts_with("GET ")),
            "{change}"
        );
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 1, "{change}");
        if matches!(
            change,
            "receipt" | "missing_origin" | "scope" | "sandbox" | "home_identity"
        ) {
            assert!(
                state.calls.is_empty(),
                "invalid local authority must not read: {change}"
            );
        }
    }
}

#[tokio::test]
async fn persisted_home_provision_checks_live_policy_and_post_assignment_home() {
    for after_dispatch in [false, true] {
        let bound = bound(true).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        if after_dispatch {
            bound.fixture.state.lock().unwrap().exec_mutations.push((
                VOLUME.into(),
                "/metadata/uid".into(),
                json!("replaced-during-assignment"),
            ));
        } else {
            bound.fixture.edit(
                "sandboxtemplates",
                &bound.claim.request.pool.template_name,
                |template| {
                    template["spec"]["networkPolicy"]["egress"] = json!([{}]);
                },
            );
        }
        assert!(bound
            .backend
            .provision_with_home(bound.request.clone(), &home)
            .await
            .is_err());
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            1 + usize::from(after_dispatch)
        );
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 1);
    }
}
