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
