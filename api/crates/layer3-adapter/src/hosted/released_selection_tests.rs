//! Native pool selection labels the image of the release new work was admitted against.
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::{HostedBackend, HostedDeployment};
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::provider::ProviderSandboxRequest;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AgentLaunch, ConversationChannel, ConversationUpdate, HomeVolumePolicy, HomeVolumeRequest,
    ReleasedRuntime, Result, RuntimeHandle, RuntimeImage, RuntimeProvisionRequest, SandboxAdapter,
    SandboxSpec,
};

#[derive(Default)]
struct RecordingBackend {
    selected: Mutex<Vec<(String, RuntimeImage)>>,
}

#[async_trait::async_trait]
impl HostedBackend for RecordingBackend {
    async fn select_native_pool(
        &self,
        request: &ProviderSandboxRequest,
        image: &RuntimeImage,
        _home: &HomeVolumeRequest,
    ) -> Result<Option<crate::agent_sandbox::NativePoolBinding>> {
        self.selected
            .lock()
            .unwrap()
            .push((request.image.uri.clone(), image.clone()));
        Ok(None)
    }
    async fn provision(&self, _request: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        unimplemented!("selection only")
    }
    async fn exchange(
        &self,
        _handle: &RuntimeHandle,
        _request: RequestFrame,
    ) -> Result<ResponseFrame> {
        unimplemented!("selection only")
    }
    async fn open_agent(
        &self,
        _handle: &RuntimeHandle,
        _launch: FencedRequest<AgentLaunch>,
        _resume_conversation: Option<String>,
        _settings: super::HostedAgentOpenSettings,
        _updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unimplemented!("selection only")
    }
    async fn teardown(&self, _handle: &RuntimeHandle) -> Result<()> {
        unimplemented!("selection only")
    }
}

#[tokio::test]
async fn pool_selection_uses_the_admitted_release_with_this_builds_contract() {
    let bootstrap = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let released = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let backend = Arc::new(RecordingBackend::default());
    let adapter = CodexAdapter::new(
        backend.clone(),
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image: crate::released_image(bootstrap.clone()),
                ..HostedDeployment::default()
            },
            ..CodexSandboxSpec::default()
        },
    );
    let spec = SandboxSpec {
        sandbox_id: "new-work".into(),
        created_by: Some("deployment".into()),
        ..SandboxSpec::default()
    };
    let home = HomeVolumeRequest {
        scope: None,
        volume_id: "home-one".into(),
        sandbox_id: spec.sandbox_id.clone(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 4,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    let admitted = RuntimeProvisionRequest::new(1, spec.clone()).with_released(ReleasedRuntime {
        revision: 3,
        agent: released.clone(),
        machine: None,
        lane_revisions: None,
    });
    adapter.select_native_pool(&admitted, &home).await.unwrap();
    adapter
        .select_native_pool(&RuntimeProvisionRequest::new(1, spec), &home)
        .await
        .unwrap();
    let selected = backend.selected.lock().unwrap().clone();
    // The admitted release renders and selects its own image and contract,
    // even though the adapter was built with an older bootstrap image.
    assert_eq!(selected[0].0, released);
    assert_eq!(selected[0].1, crate::released_image(released));
    // Work without a captured release keeps the bootstrap image.
    assert_eq!(selected[1].0, bootstrap);
    assert_eq!(selected[1].1, crate::released_image(bootstrap));
}
