use super::*;
use std::collections::BTreeMap;
#[derive(Default)]
struct Routes(tokio::sync::Mutex<BTreeMap<(String, u64), bool>>);
#[async_trait::async_trait]
impl RuntimeRoutingStore for Routes {
    async fn select(&self, id: &str, generation: u64, default_new: bool) -> Result<bool> {
        Ok(*self
            .0
            .lock()
            .await
            .entry((id.into(), generation))
            .or_insert(default_new))
    }
}

struct RecoveryBackend(&'static str);

#[async_trait::async_trait]
impl HostedBackend for RecoveryBackend {
    async fn inspect_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> Result<Option<super::super::NonexecutingRuntimeProof>> {
        Ok(Some(super::super::NonexecutingRuntimeProof {
            version: 1,
            idle_bootstrap: None,
            handle: handle.clone(),
            request_sha256: crate::provider_request_digest(request)?,
            home_identity: home.identity()?,
            sandbox_generation: 1,
            sandbox_resource_version: "1".into(),
            pod_name: self.0.into(),
            pod_uid: "owned".into(),
            pod_resource_version: "1".into(),
            runtime_container_id: "containerd://owned".into(),
            runtime_restart_count: 1,
            state_sha256: "a".repeat(64),
        }))
    }
    async fn run_recovery_command(
        &self,
        handle: &RuntimeHandle,
        command: &super::super::RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        assert_eq!(command.program, "/usr/bin/python3");
        assert_eq!(command.args, ["-I"]);
        assert_eq!(command.input, b"private-input");
        Ok(format!("{}:{}", self.0, handle.0).into_bytes())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        unreachable!("inspection cannot allocate a runtime")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        unreachable!("inspection cannot use the supervisor control API")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!("inspection cannot open an agent")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        unreachable!("inspection cannot delete a runtime")
    }
}

#[tokio::test]
async fn recovery_command_keeps_exact_runtime_route_and_never_falls_back_or_allocates() {
    let routes = Arc::new(Routes::default());
    let mut backend = RuntimeBackends {
        home_routes: None,
        agent_sandbox: Some(Arc::new(RecoveryBackend("native"))),
        opensandbox: Some(Arc::new(RecoveryBackend("legacy"))),
        allocate_agent_sandbox: true,
        routing: routes.clone(),
    };
    let command = super::super::RuntimeRecoveryCommand {
        program: "/usr/bin/python3".into(),
        args: vec!["-I".into()],
        input: b"private-input".to_vec(),
    };
    let native = RuntimeHandle::new("agent-sandbox:claimed:sandbox-uid");
    let legacy = RuntimeHandle::new("existing-provider-runtime");
    for default_new in [true, false] {
        backend.allocate_agent_sandbox = default_new;
        for (handle, provider) in [(&native, "native"), (&legacy, "legacy")] {
            assert_eq!(
                backend
                    .run_recovery_command(handle, &command)
                    .await
                    .unwrap(),
                format!("{provider}:{}", handle.0).into_bytes()
            );
        }
    }
    backend.agent_sandbox = None;
    assert!(matches!(
        backend.run_recovery_command(&native, &command).await,
        Err(AdapterError::Unavailable(_))
    ));
    backend.agent_sandbox = Some(Arc::new(super::super::InMemoryHostedBackend::default()));
    assert!(matches!(
        backend.run_recovery_command(&native, &command).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(routes.0.lock().await.is_empty());
}

#[tokio::test]
async fn unknown_creation_retry_keeps_original_backend_across_default_change() {
    let old = Arc::new(super::super::InMemoryHostedBackend::default());
    let new = Arc::new(super::super::InMemoryHostedBackend::default());
    let routes = Arc::new(Routes::default());
    let mut backend = RuntimeBackends {
        home_routes: None,
        agent_sandbox: Some(new.clone()),
        opensandbox: Some(old.clone()),
        allocate_agent_sandbox: false,
        routing: routes,
    };
    let spec = SandboxSpec {
        sandbox_id: "owned-runtime".into(),
        created_by: Some("deployment".into()),
        ..Default::default()
    };
    let request = super::super::HostedDeployment::default()
        .render(&spec)
        .unwrap();
    let original = backend.provision(request.clone()).await.unwrap();
    backend.allocate_agent_sandbox = true;
    assert_eq!(original, backend.provision(request.clone()).await.unwrap());
    assert_eq!(old.inventory().await.unwrap().len(), 1);
    assert!(new.inventory().await.unwrap().is_empty());
    let next = super::super::HostedDeployment::default()
        .render_runtime(&spec, 2)
        .unwrap();
    backend.provision(next).await.unwrap();
    assert_eq!(new.inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn home_selection_persists_provider_before_allocation_and_keeps_it_on_retry() {
    let old = Arc::new(super::super::InMemoryHostedBackend::default());
    let new = Arc::new(super::super::InMemoryHostedBackend::default());
    let mut backend = RuntimeBackends {
        home_routes: None,
        agent_sandbox: Some(new.clone()),
        opensandbox: Some(old.clone()),
        allocate_agent_sandbox: true,
        routing: Arc::new(Routes::default()),
    };
    let spec = SandboxSpec {
        sandbox_id: "selected-before-home".into(),
        created_by: Some("deployment".into()),
        ..Default::default()
    };
    let request = super::super::HostedDeployment::default()
        .render(&spec)
        .unwrap();
    let home = crate::HomeVolumeRequest {
        scope: None,
        volume_id: "home-one".into(),
        sandbox_id: spec.sandbox_id,
        policy: crate::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 4,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    assert!(backend
        .select_native_pool(
            &request,
            &super::super::HostedDeployment::default().image,
            &home
        )
        .await
        .unwrap()
        .is_none());
    backend.allocate_agent_sandbox = false;
    backend.provision(request).await.unwrap();
    assert!(old.inventory().await.unwrap().is_empty());
    assert_eq!(new.inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn missing_home_routes_reject_a_persisted_remote_scope() {
    let routes = Arc::new(Routes::default());
    let backend = RuntimeBackends {
        home_routes: None,
        agent_sandbox: Some(Arc::new(super::super::InMemoryHostedBackend::default())),
        opensandbox: Some(Arc::new(super::super::InMemoryHostedBackend::default())),
        allocate_agent_sandbox: true,
        routing: routes.clone(),
    };
    let home = HomeVolumeRequest {
        scope: Some(
            HomeVolumeScope::from_authority("kubernetes-pvc-v1", &["remote", "runtime"]).unwrap(),
        ),
        volume_id: "remote-home".into(),
        sandbox_id: "remote-sandbox".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 4,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    let handle = RuntimeHandle::new("agent-sandbox:remote-sandbox");
    assert!(backend.verify_retained_home_route(&handle, &home).is_err());
    assert!(backend.native_pools_for_home(&home).is_err());
    let request = super::super::HostedDeployment::default()
        .render(&SandboxSpec {
            sandbox_id: home.sandbox_id.clone(),
            created_by: Some("deployment".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(backend.reserve_home_request(&request, &home).await.is_err());
    assert!(backend.verify_home_route(&request, &home).await.is_err());
    assert!(backend
        .select_native_pool(
            &request,
            &super::super::HostedDeployment::default().image,
            &home
        )
        .await
        .is_err());
    assert!(
        routes.0.lock().await.is_empty(),
        "unavailable home authority cannot reserve a replacement runtime provider"
    );
}

use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::{
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeScope,
    HomeVolumeStore, RoutedHomeVolumes, RuntimeProvisionRequest, SandboxAdapter,
};

struct Homes {
    scope: HomeVolumeScope,
    rows: std::sync::Mutex<BTreeMap<String, HomeVolumeAttachment>>,
}
impl Homes {
    fn new(authority: &str) -> Arc<Self> {
        Arc::new(Self {
            scope: HomeVolumeScope::from_authority(
                "kubernetes-pvc-v1",
                &[authority, "runtime", "deployment"],
            )
            .unwrap(),
            rows: Default::default(),
        })
    }
}
#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(self.scope.clone())
    }
    async fn inventory(&self) -> Result<Vec<HomeVolumeAttachment>> {
        Ok(self.rows.lock().unwrap().values().cloned().collect())
    }
    async fn prepare(&self, request: &HomeVolumeRequest) -> Result<HomeVolumeReceipt> {
        assert!(request
            .scope
            .as_ref()
            .is_none_or(|scope| scope == &self.scope));
        let receipt = HomeVolumeReceipt {
            native_authority: None,
            volume_id: request.volume_id.clone(),
            request_sha256: request.computed_sha256(),
            storage_reference: "retained-home".into(),
            storage_generation: "original-pvc-uid".into(),
        };
        self.rows.lock().unwrap().insert(
            request.volume_id.clone(),
            HomeVolumeAttachment {
                request: request.clone(),
                receipt: receipt.clone(),
            },
        );
        Ok(receipt)
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> Result<()> {
        if self.rows.lock().unwrap().get(&home.request.volume_id) != Some(home) {
            return Err(AdapterError::Rejected("home was replaced".into()));
        }
        Ok(())
    }
    async fn remove(&self, _: &HomeVolumeRequest, _: Option<&HomeVolumeReceipt>) -> Result<()> {
        panic!("runtime recreation cannot delete a retained home")
    }
}
fn policy(seed: &str) -> HomeVolumePolicy {
    HomeVolumePolicy {
        mode: Default::default(),
        storage_class: "storage".into(),
        capacity_gib: 4,
        template_name: seed.into(),
        template_generation: format!("{seed}-uid"),
    }
}
fn home_request(id: &str) -> HomeVolumeRequest {
    HomeVolumeRequest {
        scope: None,
        volume_id: format!("home-{id}"),
        sandbox_id: id.into(),
        policy: policy("historical-seed"),
    }
}
fn runtime_request(id: &str, generation: u64) -> RuntimeProvisionRequest {
    RuntimeProvisionRequest {
        funded_until: None,
        native_canary_image: None,
        released: None,
        generation,
        spec: SandboxSpec {
            sandbox_id: id.into(),
            created_by: Some("deployment".into()),
            ..Default::default()
        },
    }
}
struct Fixture {
    local: Arc<Homes>,
    remote: Arc<Homes>,
    routes: Arc<Routes>,
    old: Arc<super::super::InMemoryHostedBackend>,
    native: Arc<super::super::InMemoryHostedBackend>,
}
impl Fixture {
    fn new() -> Self {
        Self {
            local: Homes::new("local"),
            remote: Homes::new("remote"),
            routes: Arc::new(Routes::default()),
            old: Default::default(),
            native: Default::default(),
        }
    }
    fn adapter(&self, native: bool, seed: &str) -> CodexAdapter {
        let homes = Arc::new(
            RoutedHomeVolumes::new(
                Some((self.local.clone(), policy("local-seed"))),
                Some((self.remote.clone(), policy(seed))),
            )
            .unwrap(),
        );
        CodexAdapter::with_backend(
            RuntimeBackends {
                home_routes: Some(homes.clone()),
                opensandbox: Some(self.old.clone()),
                agent_sandbox: Some(self.native.clone()),
                allocate_agent_sandbox: native,
                routing: self.routes.clone(),
            },
            CodexSandboxSpec::default(),
        )
        .with_home_volumes(homes)
    }
}

#[tokio::test]
async fn cold_home_reservation_survives_adapter_reopen_default_and_seed_changes() {
    let fixture = Fixture::new();
    let original = fixture.adapter(true, "remote-seed");
    let mut runtime = runtime_request("cold-native", 1);
    let selected = original
        .reserve_home_request(&runtime, &home_request("cold-native"))
        .await
        .unwrap();
    assert_eq!(selected.scope.as_ref(), Some(&fixture.remote.scope));
    assert_eq!(selected.policy, policy("remote-seed"));
    assert!(fixture.local.inventory().await.unwrap().is_empty());
    assert!(fixture.remote.inventory().await.unwrap().is_empty());
    let persisted = serde_json::to_vec(&selected).unwrap();
    let reopened = fixture.adapter(false, "replacement-seed");
    let selected: HomeVolumeRequest = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(
        reopened
            .reserve_home_request(&runtime, &selected)
            .await
            .unwrap(),
        selected
    );
    let store = reopened.home_volumes().unwrap();
    runtime.spec.home_volume = Some(Box::new(HomeVolumeAttachment {
        receipt: store.prepare(&selected).await.unwrap(),
        request: selected,
    }));
    reopened.provision_runtime(&runtime).await.unwrap();
    assert!(fixture.old.inventory().await.unwrap().is_empty());
    assert_eq!(fixture.native.inventory().await.unwrap().len(), 1);
    assert!(fixture.local.inventory().await.unwrap().is_empty());
}

#[tokio::test]
async fn retained_home_recreation_uses_original_provider_after_default_changes() {
    for native in [false, true] {
        let fixture = Fixture::new();
        let first = fixture.adapter(native, "remote-seed");
        let mut runtime = runtime_request("retained", 1);
        let home = if native {
            first
                .reserve_home_request(&runtime, &home_request("retained"))
                .await
                .unwrap()
        } else {
            home_request("retained") // pre-migration request, including its seed and absent scope
        };
        runtime.spec.home_volume = Some(Box::new(HomeVolumeAttachment {
            receipt: first.home_volumes().unwrap().prepare(&home).await.unwrap(),
            request: home.clone(),
        }));
        let handle = first.provision_runtime(&runtime).await.unwrap();
        let selected = if native {
            &fixture.native
        } else {
            &fixture.old
        };
        selected.teardown(&handle).await.unwrap();
        let reopened = fixture.adapter(!native, "changed-seed");
        runtime.generation = 2;
        reopened.provision_runtime(&runtime).await.unwrap();
        assert_eq!(runtime.spec.home_volume.as_ref().unwrap().request, home);
        assert_eq!(
            fixture.routes.0.lock().await.get(&("retained".into(), 2)),
            Some(&native)
        );
        let (selected, other) = if native {
            (&fixture.native, &fixture.old)
        } else {
            (&fixture.old, &fixture.native)
        };
        assert_eq!(selected.inventory().await.unwrap().len(), 1);
        assert!(other.inventory().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn durable_provider_conflict_refuses_both_runtime_dispatch_paths_before_backend_creation() {
    let fixture = Fixture::new();
    let adapter = fixture.adapter(true, "remote-seed");
    let mut runtime = runtime_request("conflict", 2);
    let home = home_request("conflict");
    runtime.spec.home_volume = Some(Box::new(HomeVolumeAttachment {
        receipt: adapter
            .home_volumes()
            .unwrap()
            .prepare(&home)
            .await
            .unwrap(),
        request: home,
    }));
    fixture.routes.select("conflict", 2, true).await.unwrap();
    assert!(matches!(
        adapter.provision_runtime(&runtime).await,
        Err(AdapterError::Rejected(_))
    ));
    assert!(matches!(
        adapter
            .provision_runtime_receipted(&runtime, "original-dispatch")
            .await,
        Err(AdapterError::Rejected(_))
    ));
    assert!(fixture.old.inventory().await.unwrap().is_empty());
    assert!(fixture.native.inventory().await.unwrap().is_empty());
    assert!(adapter
        .native_pools_for_home(&home_request("legacy"))
        .unwrap()
        .is_none());
    let mut unknown = home_request("unknown");
    unknown.scope = Some(Homes::new("foreign").scope.clone());
    assert!(adapter.native_pools_for_home(&unknown).is_err());
}

#[tokio::test]
async fn retained_runtime_adoption_suspension_and_resume_refuse_cross_cluster_home_without_new_routes(
) {
    for native_home in [false, true] {
        let fixture = Fixture::new();
        let adapter = fixture.adapter(native_home, "remote-seed");
        let mut runtime = runtime_request("retained-mismatch", 1);
        let home = if native_home {
            adapter
                .reserve_home_request(&runtime, &home_request("retained-mismatch"))
                .await
                .unwrap()
        } else {
            home_request("retained-mismatch")
        };
        runtime.spec.home_volume = Some(Box::new(HomeVolumeAttachment {
            receipt: adapter
                .home_volumes()
                .unwrap()
                .prepare(&home)
                .await
                .unwrap(),
            request: home,
        }));
        let handle = RuntimeHandle::new(if native_home {
            "old-provider-runtime"
        } else {
            "agent-sandbox:namespace:name:uid"
        });
        let retained = crate::RuntimeAdoptionRequest::new(1, handle, runtime.spec);
        let route_count = fixture.routes.0.lock().await.len();
        for result in [
            adapter.adopt_runtime(&retained).await.map(|_| ()),
            adapter.suspend_runtime(&retained).await,
            adapter.verify_suspended_runtime(&retained).await,
            adapter
                .resume_runtime(
                    &retained,
                    &RuntimeProvisionRequest::new(2, retained.spec.clone()),
                )
                .await
                .map(|_| ()),
        ] {
            assert!(
                matches!(result, Err(AdapterError::Rejected(message)) if message == "retained runtime conflicts with its recorded home authority")
            );
        }
        assert_eq!(fixture.routes.0.lock().await.len(), route_count);
        assert!(fixture.old.inventory().await.unwrap().is_empty());
        assert!(fixture.native.inventory().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn nonexecuting_proof_keeps_runtime_route_and_never_falls_back_or_allocates() {
    let routes = Arc::new(Routes::default());
    let mut backend = RuntimeBackends {
        home_routes: None,
        agent_sandbox: Some(Arc::new(RecoveryBackend("native"))),
        opensandbox: Some(Arc::new(RecoveryBackend("legacy"))),
        allocate_agent_sandbox: false,
        routing: routes.clone(),
    };
    let request = crate::hosted::HostedDeployment::default()
        .render_runtime(&runtime_request("owned", 1).spec, 1)
        .unwrap();
    let home_request = home_request("owned");
    let home = HomeVolumeAttachment {
        receipt: HomeVolumeReceipt {
            native_authority: None,
            volume_id: home_request.volume_id.clone(),
            request_sha256: home_request.computed_sha256(),
            storage_reference: "original".into(),
            storage_generation: "original-uid".into(),
        },
        request: home_request,
    };
    let native = RuntimeHandle::new("agent-sandbox:owned:sandbox-uid");
    for default_new in [false, true] {
        backend.allocate_agent_sandbox = default_new;
        for (handle, provider) in [
            (&native, "native"),
            (&RuntimeHandle::new("existing-legacy"), "legacy"),
        ] {
            let proof = backend
                .inspect_nonexecuting_runtime(handle, &request, &home)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(proof.pod_name, provider);
            backend
                .verify_nonexecuting_runtime(handle, &request, &home, &proof)
                .await
                .unwrap();
        }
    }
    backend.agent_sandbox = None;
    assert!(matches!(
        backend
            .inspect_nonexecuting_runtime(&native, &request, &home)
            .await,
        Err(AdapterError::Unavailable(_))
    ));
    backend.agent_sandbox = Some(Arc::new(super::super::InMemoryHostedBackend::default()));
    assert!(matches!(
        backend
            .inspect_nonexecuting_runtime(&native, &request, &home)
            .await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(routes.0.lock().await.is_empty());
}
