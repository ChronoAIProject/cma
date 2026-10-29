use super::*;
use crate::{
    agent_sandbox::AgentSandboxClient,
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::{
        AgentSandboxBackend, HostedAgentOpenSettings, HostedControlTransport, HostedDeployment,
    },
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle, RuntimeImage,
    RuntimeProvisionRequest, SandboxAdapter, SandboxCapacity, SandboxSpec,
};
use std::sync::Arc;

struct NoRuntimeTransport;
#[async_trait::async_trait]
impl HostedControlTransport for NoRuntimeTransport {
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("pool selection cannot contact a runtime")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("pool selection cannot open an Agent")
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("pool selection cannot retire a runtime")
    }
}

fn adapter(fixture: &Fixture, image: RuntimeImage) -> CodexAdapter {
    let client = AgentSandboxClient::new(
        fixture.kubernetes.clone(),
        fixture.client.config.clone(),
        "fixture",
    )
    .unwrap();
    CodexAdapter::new(
        Arc::new(AgentSandboxBackend::new(
            client,
            Arc::new(NoRuntimeTransport),
        )),
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image,
                ..Default::default()
            },
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn codex_selects_registered_desktop_image_without_changing_headless_or_admitting_other_images(
) {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().rows.remove(seed_path());
    let headless = spec();
    let mut desktop = headless.image.clone();
    desktop.uri = format!("registry.example/desktop@sha256:{}", "b".repeat(64));
    let home = crate::HomeVolumeRequest {
        volume_id: "home-one".into(),
        sandbox_id: "sbx_one".into(),
        policy: headless.home.clone(),
        scope: homes(&fixture).allocation_scope(),
    };
    let mut request = RuntimeProvisionRequest::new(
        1,
        SandboxSpec {
            sandbox_id: home.sandbox_id.clone(),
            created_by: Some("cma-test".into()),
            runtime_image: Some(desktop.uri.clone()),
            capacity: SandboxCapacity {
                cpu_millis: 2000,
                memory_mib: 8192,
            },
            ..Default::default()
        },
    );
    let adapter = adapter(&fixture, headless.image.clone());
    assert!(adapter.select_native_pool(&request, &home).await.is_err());
    assert!(fixture.state.lock().unwrap().calls.is_empty());
    let adapter = adapter.with_native_runtime_image(desktop.clone()).unwrap();
    let selected = adapter
        .select_native_pool(&request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(selected.spec.image, desktop);
    assert_eq!(
        (selected.spec.cpu_millis, selected.spec.memory_mib),
        (2000, 8192)
    );
    assert_eq!(selected.spec.home, home.policy);
    let template = fixture.row("sandboxtemplates", &selected.template_name);
    assert_eq!(
        template["spec"]["podTemplate"]["spec"]["containers"][0]["image"],
        desktop.uri
    );
    assert_eq!(
        fixture.row("sandboxwarmpools", &selected.name)["spec"]["replicas"],
        0
    );
    request.spec.runtime_image = None;
    let original = adapter
        .select_native_pool(&request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.spec.image, headless.image);
    assert_ne!(original.name, selected.name);
    let before = fixture.state.lock().unwrap().calls.len();
    request.spec.runtime_image = Some(format!("registry.example/other@sha256:{}", "c".repeat(64)));
    assert!(adapter.select_native_pool(&request, &home).await.is_err());
    assert_eq!(fixture.state.lock().unwrap().calls.len(), before);
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn registering_a_native_image_requires_its_own_pinned_contract_and_bootstrap_evidence() {
    let fixture = Fixture::new().await;
    let headless = spec().image;
    for change in ["unpinned", "contract", "bootstrap"] {
        let mut image = headless.clone();
        match change {
            "unpinned" => image.uri = "registry.example/desktop:latest".into(),
            "contract" => {
                image.labels.remove(crate::RUNTIME_CONTRACT_LABEL);
            }
            _ => {
                image
                    .labels
                    .remove(crate::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL);
            }
        }
        assert!(adapter(&fixture, headless.clone())
            .with_native_runtime_image(image)
            .is_err());
    }
    assert!(fixture.state.lock().unwrap().calls.is_empty());
}
