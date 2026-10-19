use super::*;
use crate::hosted::{HostedAgentOpenSettings, HostedBackend, HostedDeployment};
use crate::{RuntimeAdoptionRequest, RuntimeProvisionRequest, SandboxAdapter};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct UpgradeBackend(
    Mutex<
        Vec<(
            ProviderSandboxRequest,
            ProviderSandboxRequest,
            crate::HomeVolumeAttachment,
        )>,
    >,
);
#[async_trait::async_trait]
impl HostedBackend for UpgradeBackend {
    async fn upgrade_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &crate::HomeVolumeAttachment,
    ) -> crate::Result<()> {
        assert_eq!(handle.as_str(), "agent-sandbox:warm-member:member-uid");
        self.0
            .lock()
            .unwrap()
            .push((previous.clone(), next.clone(), home.clone()));
        Ok(())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        panic!("upgrade retains the runtime")
    }
    async fn exchange(
        &self,
        _: &RuntimeHandle,
        _: crate::wire::RequestFrame,
    ) -> crate::Result<crate::wire::ResponseFrame> {
        panic!("upgrade has no direct control request")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("upgrade retains the runtime")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: crate::wire::FencedRequest<crate::AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<crate::ConversationUpdate>,
    ) -> crate::Result<(
        crate::ConversationChannel,
        crate::wire::FencedReceipt<crate::wire::AgentOpenReceipt>,
    )> {
        panic!("core owns conversation reopen")
    }
}

#[tokio::test]
async fn native_upgrade_hosted_dispatch_pins_target_and_refuses_unrelated_or_unrecorded_changes() {
    let fixture = Resume::new().await;
    let backend = Arc::new(UpgradeBackend::default());
    let adapter = crate::codex::CodexAdapter::new(
        backend.clone(),
        crate::codex::CodexSandboxSpec {
            deployment: HostedDeployment {
                image: crate::RuntimeImage {
                    uri: image('c'),
                    ..HostedDeployment::default().image
                },
                ..HostedDeployment::default()
            },
            ..Default::default()
        },
    )
    .with_home_volumes(Arc::new(homes(&fixture.fixture)));
    let pool = &fixture
        .home
        .receipt
        .native_authority
        .as_ref()
        .unwrap()
        .claim
        .request
        .pool
        .spec;
    let spec = crate::SandboxSpec {
        sandbox_id: fixture.home.request.sandbox_id.clone(),
        runtime_image: Some(fixture.previous.image.uri.clone()),
        home_volume: Some(Box::new(fixture.home.clone())),
        created_by: Some("cma-test".into()),
        capacity: crate::SandboxCapacity {
            cpu_millis: pool.cpu_millis,
            memory_mib: pool.memory_mib,
        },
        ..Default::default()
    };
    let previous = RuntimeAdoptionRequest::new(1, fixture.handle.clone(), spec)
        .with_expected_image(fixture.previous.image.uri.clone());
    let mut next = RuntimeProvisionRequest::new(2, previous.spec.clone());
    next.spec.runtime_image = Some(image('b'));
    for field in [
        "generation",
        "home",
        "image",
        "environment",
        "capacity",
        "owner",
        "expected_image",
        "previous_image",
    ] {
        let mut invalid = next.clone();
        let mut old = previous.clone();
        match field {
            "generation" => invalid.generation = 1,
            "home" => invalid.spec.home_volume = None,
            "image" => invalid.spec.runtime_image = None,
            "environment" => {
                invalid
                    .spec
                    .environment
                    .insert("USER".into(), "foreign".into());
            }
            "capacity" => invalid.spec.capacity.cpu_millis += 1,
            "owner" => invalid.spec.created_by = Some("foreign".into()),
            "expected_image" => old.expected_image = None,
            "previous_image" => old.expected_image = Some(image('c')),
            _ => unreachable!(),
        }
        assert!(
            adapter.upgrade_runtime(&old, &invalid).await.is_err(),
            "{field}"
        );
    }
    assert!(adapter.resume_runtime(&previous, &next).await.is_err());
    assert!(backend.0.lock().unwrap().is_empty());
    for _ in 0..2 {
        assert_eq!(
            adapter.upgrade_runtime(&previous, &next).await.unwrap(),
            previous.handle
        );
    }
    let calls = backend.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(calls[0].0, fixture.previous);
    assert_eq!(calls[0].1.image.uri, image('b'));
    assert_eq!(calls[0].1.metadata["cma/runtime-generation"], "2");
    assert_eq!(calls[0].1.volumes, fixture.previous.volumes);
    assert_eq!(calls[0].2, fixture.home);
}

struct RetainedRoute;
#[async_trait::async_trait]
impl crate::hosted::RuntimeRoutingStore for RetainedRoute {
    async fn select(&self, _: &str, _: u64, _: bool) -> crate::Result<bool> {
        panic!("upgrade cannot allocate a new provider")
    }
}
#[tokio::test]
async fn native_upgrade_backend_dispatch_keeps_recorded_provider_when_default_changes() {
    let fixture = Resume::new().await;
    let backend = Arc::new(UpgradeBackend::default());
    let routes = crate::hosted::RuntimeBackends {
        agent_sandbox: Some(backend.clone()),
        opensandbox: None,
        allocate_agent_sandbox: false,
        routing: Arc::new(RetainedRoute),
        home_routes: None,
    };
    let next = target(&fixture.home, 2, &image('b'));
    routes
        .upgrade_with_home(&fixture.handle, &fixture.previous, &next, &fixture.home)
        .await
        .unwrap();
    assert_eq!(backend.0.lock().unwrap().len(), 1);
}
