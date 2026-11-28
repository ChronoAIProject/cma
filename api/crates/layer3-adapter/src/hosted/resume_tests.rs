use super::*;
use crate::hosted::{HostedAgentOpenSettings, HostedBackend, HostedDeployment};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeAdoptionRequest, RuntimeHandle,
    RuntimeProvisionRequest,
};
use std::sync::Mutex;
use tokio::sync::mpsc;

#[derive(Default)]
struct ResumeBackend(Mutex<Vec<(ProviderSandboxRequest, ProviderSandboxRequest)>>);

#[async_trait::async_trait]
impl HostedBackend for ResumeBackend {
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> crate::Result<()> {
        assert_eq!(handle.as_str(), "retained-runtime");
        self.0
            .lock()
            .unwrap()
            .push((previous.clone(), next.clone()));
        Ok(())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        panic!("resume must not create another runtime")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("resume must not issue a control request")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("resume must not open an Agent")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("resume must not delete the retained runtime")
    }
}

fn fixture() -> (CodexAdapter, Arc<ResumeBackend>, RuntimeAdoptionRequest) {
    let backend = Arc::new(ResumeBackend::default());
    let adapter = CodexAdapter::new(
        backend.clone(),
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image: crate::RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "b".repeat(64)),
                    ..HostedDeployment::default().image
                },
                upstream_base_url: "https://nyx.example/api/v1/proxy/s/chrono-llm-public".into(),
                platform_base_url: "https://nyx.example".into(),
                ..HostedDeployment::default()
            },
            ..CodexSandboxSpec::default()
        },
    )
    .with_home_volumes(Arc::new(Storage::default()));
    let mut spec = spec();
    spec.reaches = vec!["chrono-llm".into()];
    let retained = RuntimeAdoptionRequest::new(2, RuntimeHandle::new("retained-runtime"), spec)
        .with_expected_image(format!(
            "registry.example/runtime@sha256:{}",
            "a".repeat(64)
        ));
    (adapter, backend, retained)
}

#[tokio::test]
async fn scope_resume_renders_new_scope_and_keeps_persisted_image_and_home_on_replay() {
    let (adapter, backend, retained) = fixture();
    let mut next = RuntimeProvisionRequest::new(3, retained.spec.clone());
    next.spec
        .reaches
        .extend(["chrono-llm-public".into(), "api-github".into()]);
    for _ in 0..2 {
        assert_eq!(
            adapter.resume_runtime(&retained, &next).await.unwrap(),
            retained.handle
        );
    }
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    let (previous, resumed) = &calls[0];
    assert_eq!(
        previous.image.uri,
        retained.expected_image.as_deref().unwrap()
    );
    assert_eq!(resumed.image, previous.image);
    assert_eq!(resumed.volumes, previous.volumes);
    assert_eq!(
        resumed.metadata["cma/home-identity"],
        previous.metadata["cma/home-identity"]
    );
    assert_eq!(previous.env["CHRONO_SANDBOX_REACHES"], "chrono-llm");
    assert_eq!(
        resumed.env["CHRONO_SANDBOX_REACHES"],
        "chrono-llm,chrono-llm-public,api-github"
    );
    assert_eq!(
        previous.env["CHRONO_UPSTREAM_BASE_URL"],
        "https://nyx.example/api/v1/proxy/s/chrono-llm"
    );
    assert_eq!(
        resumed.env["CHRONO_UPSTREAM_BASE_URL"],
        "https://nyx.example/api/v1/proxy/s/chrono-llm-public"
    );
    assert_eq!(resumed.metadata["cma/runtime-generation"], "3");
    assert_ne!(
        resumed.metadata["cma/spec-sha256"],
        previous.metadata["cma/spec-sha256"]
    );
}

#[tokio::test]
async fn resume_checks_the_provider_when_this_replica_cached_an_older_generation() {
    let (adapter, backend, mut retained) = fixture();
    let cached = RuntimeProvisionRequest::new(3, retained.spec.clone());
    adapter.resume_runtime(&retained, &cached).await.unwrap();

    // Another replica suspended generation 3 and resumed generation 4.
    // This replica still knows generation 3 under the same retained handle.
    retained.generation = 4;
    let next = RuntimeProvisionRequest::new(5, retained.spec.clone());
    assert_eq!(
        adapter.resume_runtime(&retained, &next).await.unwrap(),
        retained.handle
    );

    let calls = backend.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0.metadata["cma/runtime-generation"], "4");
    assert_eq!(calls[1].1.metadata["cma/runtime-generation"], "5");
}

#[tokio::test]
async fn resume_refuses_a_cached_successor_with_a_different_specification() {
    let (adapter, backend, retained) = fixture();
    let cached = RuntimeProvisionRequest::new(3, retained.spec.clone());
    adapter.resume_runtime(&retained, &cached).await.unwrap();

    let mut different = cached;
    different.spec.reaches.push("api-github".into());
    assert!(matches!(
        adapter.resume_runtime(&retained, &different).await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(backend.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn scope_resume_refuses_unreserved_generation_and_unrelated_spec_changes_before_dispatch() {
    let (adapter, backend, retained) = fixture();
    let next = RuntimeProvisionRequest::new(3, retained.spec.clone());
    for field in [
        "generation",
        "sandbox",
        "capacity",
        "image",
        "environment",
        "credential",
        "home",
        "tool",
        "supervisor",
        "timeout",
        "owner",
    ] {
        let mut changed = next.clone();
        match field {
            "generation" => changed.generation = retained.generation,
            "sandbox" => changed.spec.sandbox_id = "other".into(),
            "capacity" => changed.spec.capacity.cpu_millis += 1,
            "image" => changed.spec.runtime_image = retained.expected_image.clone(),
            "environment" => {
                changed
                    .spec
                    .environment
                    .insert("USER".into(), "other".into());
            }
            "credential" => changed.spec.own_credential = true,
            "home" => {
                changed
                    .spec
                    .home_volume
                    .as_mut()
                    .unwrap()
                    .receipt
                    .storage_generation = "replacement".into()
            }
            "tool" => changed.spec.tool_proxy_url = Some("https://other.example/mcp".into()),
            "supervisor" => changed.spec.supervisor_program = "/other".into(),
            "timeout" => changed.spec.timeout_seconds = Some(1),
            "owner" => changed.spec.created_by = Some("other".into()),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                adapter.resume_runtime(&retained, &changed).await,
                Err(AdapterError::Rejected(_))
            ),
            "{field}"
        );
    }
    assert!(backend.0.lock().unwrap().is_empty());
}

struct HomeResumeBackend {
    storage: Arc<Storage>,
    replace_after_resume: bool,
    homes: Mutex<Vec<HomeVolumeAttachment>>,
}

#[async_trait::async_trait]
impl HostedBackend for HomeResumeBackend {
    async fn resume_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> crate::Result<()> {
        assert_eq!(handle.as_str(), "agent-sandbox:retained:original");
        assert_eq!(
            previous.metadata["cma/home-identity"],
            home.identity().unwrap()
        );
        assert_eq!(next.metadata["cma/home-identity"], home.identity().unwrap());
        self.homes.lock().unwrap().push(home.clone());
        if self.replace_after_resume {
            self.storage.missing.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        panic!("resume only")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("resume only")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("resume only")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("resume only")
    }
}

struct NoNewRoute;
#[async_trait::async_trait]
impl crate::hosted::RuntimeRoutingStore for NoNewRoute {
    async fn select(&self, _: &str, _: u64, _: bool) -> crate::Result<bool> {
        panic!("resume must keep its recorded provider")
    }
}

#[tokio::test]
async fn contextual_resume_routes_exact_home_and_keeps_pre_and_post_home_checks() {
    for phase in ["valid", "before", "after"] {
        let storage = Arc::new(Storage::default());
        storage.missing.store(phase == "before", Ordering::SeqCst);
        let backend = Arc::new(HomeResumeBackend {
            storage: storage.clone(),
            replace_after_resume: phase == "after",
            homes: Mutex::new(vec![]),
        });
        let routes = crate::hosted::RuntimeBackends {
            agent_sandbox: Some(backend.clone()),
            opensandbox: None,
            allocate_agent_sandbox: false,
            routing: Arc::new(NoNewRoute),
            home_routes: None,
        };
        let adapter = CodexAdapter::new(Arc::new(routes), CodexSandboxSpec::default())
            .with_home_volumes(storage);
        let retained = RuntimeAdoptionRequest::new(
            1,
            RuntimeHandle::new("agent-sandbox:retained:original"),
            spec(),
        );
        let successor = RuntimeProvisionRequest::new(2, retained.spec.clone());
        let result = adapter.resume_runtime(&retained, &successor).await;
        if phase == "valid" {
            assert_eq!(result.unwrap(), retained.handle);
        } else {
            assert!(matches!(result, Err(AdapterError::Rejected(_))), "{phase}");
        }
        let homes = backend.homes.lock().unwrap();
        if phase == "before" {
            assert!(homes.is_empty());
        } else {
            assert_eq!(homes.as_slice(), std::slice::from_ref(&attachment()));
        }
    }
}
