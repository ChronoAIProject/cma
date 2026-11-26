use super::*;
use layer2_core::sandbox::{SandboxHome, SandboxNativePool};
use layer3_adapter::{
    agent_sandbox::{
        AgentSandboxClient, AgentSandboxConfig, NativeFsGroupChangePolicy, NativeHomeVolumes,
        NativePoolClaimRequest, NativePoolSpec,
    },
    hosted::{AgentSandboxBackend, HostedDeployment},
    HomeVolumeMode, HomeVolumePolicy, HomeVolumeRequest, HomeVolumeStore, RuntimeAdoptionRequest,
};

pub(super) struct NativeFixture {
    pub api: api::Api,
    pub adapter: Arc<CodexAdapter>,
    pub transport: Arc<transport::StatusTransport>,
    pub claim_path: String,
}

impl NativeFixture {
    pub async fn new(sandbox: &mut Sandbox) -> Self {
        Self::with_fs_group_policy(sandbox, None).await
    }

    pub async fn with_fs_group_policy(
        sandbox: &mut Sandbox,
        policy: Option<NativeFsGroupChangePolicy>,
    ) -> Self {
        let api = api::Api::new().await;
        let config = AgentSandboxConfig {
            namespace: "cma-test".into(),
            deployment: "test".into(),
            runtime_class: None,
            service_account: "cma-runtime".into(),
            image_pull_secrets: Vec::new(),
            template_network_policy: None,
            template_fs_group_change_policy: policy,
            native_claim_guard_enabled: false,
            runtime_startup_breaker_enabled: false,
            control_key: "fixture".repeat(8),
        };
        let client =
            AgentSandboxClient::new(api.client.clone(), config.clone(), "fixture").unwrap();
        let homes =
            Arc::new(NativeHomeVolumes::new(api.client.clone(), config, "fixture").unwrap());
        let mut deployment = HostedDeployment::default();
        // The generic in-memory fixture uses a tag; native pool authority
        // requires a digest before this record is inserted into the test store.
        deployment.image.uri = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
        sandbox.runtime.as_mut().unwrap().runtime_version = deployment.image.uri.clone();
        deployment.image.labels.insert(
            layer3_adapter::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL.into(),
            "1".into(),
        );
        deployment.image.labels.insert(
            layer3_adapter::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
            "2".into(),
        );
        let spec = crate::sandbox::runtime_mapping::sandbox_spec(sandbox, "test").unwrap();
        let pool = client
            .warm_pools()
            .configure(
                &NativePoolSpec {
                    cold_canary: false,
                    image: deployment.image.clone(),
                    cpu_millis: spec.capacity.cpu_millis,
                    memory_mib: spec.capacity.memory_mib,
                    home: HomeVolumePolicy {
                        mode: HomeVolumeMode::NativeVctEmptyV1,
                        storage_class: "cma-home".into(),
                        capacity_gib: 4,
                        template_name: String::new(),
                        template_generation: String::new(),
                    },
                },
                1,
            )
            .await
            .unwrap()
            .binding;
        let claim = client
            .warm_pools()
            .claim(&NativePoolClaimRequest {
                home: HomeVolumeRequest {
                    scope: homes.allocation_scope(),
                    volume_id: sandbox
                        .lifecycle
                        .home
                        .as_ref()
                        .unwrap()
                        .request
                        .volume_id
                        .clone(),
                    sandbox_id: sandbox.id.to_string(),
                    policy: pool.spec.home.clone(),
                },
                pool: pool.clone(),
                runtime_generation: 1,
            })
            .await
            .unwrap();
        let claim_path = format!("{}/sandboxclaims/{}", api::PREFIX, claim.name);
        {
            let mut state = api.state.lock().unwrap();
            state.rows.get_mut(&claim_path).unwrap()["status"] =
                json!({"sandbox":{"name":"warm-member"}});
            let mut blueprint = state.rows
                [&format!("{}/sandboxtemplates/{}", api::PREFIX, pool.template_name)]["spec"]
                .clone();
            for field in [
                "volumeClaimTemplatesPolicy",
                "networkPolicyManagement",
                "networkPolicy",
            ] {
                blueprint.as_object_mut().unwrap().remove(field);
            }
            blueprint["podTemplate"]["metadata"]["labels"]
                ["agents.x-k8s.io/sandbox-template-ref-hash"] =
                json!(name_hash(&pool.template_name));
            blueprint["operatingMode"] = json!("Running");
            let mut volume = blueprint["volumeClaimTemplates"][0].clone();
            volume["apiVersion"] = json!("v1");
            volume["kind"] = json!("PersistentVolumeClaim");
            volume["metadata"]["name"] = json!("home-warm-member");
            volume["metadata"]["namespace"] = json!("cma-test");
            volume["metadata"]["uid"] = json!("pvc-uid");
            volume["metadata"]["resourceVersion"] = json!("51");
            volume["metadata"]["ownerReferences"] = json!([{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true,"blockOwnerDeletion":true}]);
            state.rows.insert(api::VOLUME.into(), volume);
            state.rows.insert(api::MEMBER.into(), json!({
                "apiVersion":"agents.x-k8s.io/v1beta1", "kind":"Sandbox",
                "metadata":{"name":"warm-member","namespace":"cma-test","uid":"member-uid","resourceVersion":"50","generation":1,
                    "ownerReferences":[{"apiVersion":"extensions.agents.x-k8s.io/v1beta1","kind":"SandboxClaim","name":claim.name,"uid":claim.uid,"controller":true,"blockOwnerDeletion":true}]},
                "spec":blueprint, "status":{"conditions":[{"type":"Ready","status":"True","observedGeneration":1}]}
            }));
            state.rows.insert(api::POD.into(), json!({
                "apiVersion":"v1","kind":"Pod",
                "metadata":{"name":"warm-member","namespace":"cma-test","uid":"original-pod",
                    "labels":{"cma/control-plane":"test","agents.x-k8s.io/sandbox-name-hash":name_hash("warm-member")},
                    "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"warm-member","uid":"member-uid","controller":true}]},
                "status":{"phase":"Running","podIP":"10.0.0.7","conditions":[{"type":"Ready","status":"True"}],
                    "containerStatuses":[{"name":"runtime","image":"runtime","imageID":"image-id","containerID":"containerd://original","ready":true,"restartCount":0,"state":{"running":{}}}]}
            }));
        }
        let claim = client.warm_pools().observe_claim(&claim).await.unwrap();
        let mut home = None;
        for _ in 0..5 {
            match client.warm_pools().adopt_home(&claim).await {
                Ok(ready) => {
                    home = Some(ready);
                    break;
                }
                Err(layer3_adapter::AdapterError::Unavailable(_)) => {
                    let mut state = api.state.lock().unwrap();
                    let member = state.rows.get_mut(api::MEMBER).unwrap();
                    member["status"]["conditions"][0]["observedGeneration"] =
                        member["metadata"]["generation"].clone();
                }
                Err(error) => panic!("native fixture home refused: {error}"),
            }
        }
        let home = home.expect("native fixture home must settle");
        sandbox.lifecycle.home = Some(Box::new(SandboxHome {
            request: home.request.clone(),
            receipt: Some(home.receipt.clone()),
            released: false,
            native_pool: Some(Box::new(SandboxNativePool {
                request: claim.request.clone(),
                claim: Some(claim.clone()),
                cleanup: None,
                cleaned: false,
            })),
        }));
        let spec = crate::sandbox::runtime_mapping::sandbox_spec(sandbox, "test").unwrap();
        let request = deployment.render_runtime(&spec, 1).unwrap();
        let handle = client
            .admit_pool_runtime(&claim, &home, &request)
            .await
            .unwrap();
        let binding = sandbox.runtime.as_mut().unwrap();
        binding.runtime_id = handle.0.clone();
        binding.boot_id = Some("a".repeat(32));
        let image = binding.runtime_version.clone();
        sandbox.validate().unwrap();
        let transport = Arc::new(transport::StatusTransport::new(
            client.clone(),
            api.state.clone(),
        ));
        let adapter = Arc::new(
            CodexAdapter::new(
                Arc::new(AgentSandboxBackend::new(client, transport.clone())),
                CodexSandboxSpec {
                    deployment,
                    ..Default::default()
                },
            )
            .with_home_volumes(homes),
        );
        adapter
            .adopt_runtime(
                &RuntimeAdoptionRequest::new(1, handle, spec).with_expected_image(&image),
            )
            .await
            .unwrap()
            .unwrap();
        Self {
            api,
            adapter,
            transport,
            claim_path,
        }
    }

    pub fn mutation(&self, change: &str) -> (String, &'static str, serde_json::Value) {
        match change {
            "none" => (api::POD.into(), "/metadata/uid", json!(null)),
            "claim_uid" => (
                self.claim_path.clone(),
                "/metadata/uid",
                json!("replacement"),
            ),
            "assignment" => (
                self.claim_path.clone(),
                "/status/sandbox/name",
                json!("other-member"),
            ),
            "pod" => (api::POD.into(), "/metadata/uid", json!("replaced-pod")),
            "container" => (
                api::POD.into(),
                "/status/containerStatuses/0/containerID",
                json!("containerd://replacement"),
            ),
            "pvc" => (api::VOLUME.into(), "/metadata/uid", json!("replaced-pvc")),
            "image" => (
                api::MEMBER.into(),
                "/spec/podTemplate/spec/containers/0/image",
                json!("replacement-image"),
            ),
            "generation" => (
                api::MEMBER.into(),
                "/metadata/annotations/cma~1runtime-generation",
                json!("42"),
            ),
            _ => unreachable!(),
        }
    }
}

fn name_hash(name: &str) -> String {
    // The upstream controller's FNV-1a discovery label; ownership still uses UID.
    let hash = name.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    format!("{hash:08x}")
}
