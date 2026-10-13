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
#[tokio::test]
async fn unknown_creation_retry_keeps_original_backend_across_default_change() {
    let old = Arc::new(super::super::InMemoryHostedBackend::default());
    let new = Arc::new(super::super::InMemoryHostedBackend::default());
    let routes = Arc::new(Routes::default());
    let mut backend = RuntimeBackends {
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
        volume_id: "home-one".into(),
        sandbox_id: spec.sandbox_id,
        policy: crate::HomeVolumePolicy {
            storage_class: "storage".into(),
            capacity_gib: 4,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    assert!(backend
        .select_native_pool(&request, &home)
        .await
        .unwrap()
        .is_none());
    backend.allocate_agent_sandbox = false;
    backend.provision(request).await.unwrap();
    assert!(old.inventory().await.unwrap().is_empty());
    assert_eq!(new.inventory().await.unwrap().len(), 1);
}
