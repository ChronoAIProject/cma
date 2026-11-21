//! HTTP projection fixture; production transactional lifecycle coverage lives in
//! cma/store/bindings/reprovision/lifecycle_http_tests.rs.
use super::*;
use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};

pub(in crate::rest) const KEY: &str = "nyx_public_contract_preparation";

struct PreparationCustody {
    store: Arc<InMemorySandboxStore>,
    receipts: tokio::sync::Mutex<std::collections::HashMap<String, BindingPreparationReceipt>>,
}

#[async_trait::async_trait]
impl BindingCredentialCustody for PreparationCustody {
    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        unreachable!()
    }

    async fn retain_binding(
        &self,
        binding: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        binding.require_credential_admission(key)
    }

    async fn prepare_retired_binding(
        &self,
        intent: &BindingPreparationIntent,
        key: &ProvidedCredential,
    ) -> Result<BindingPreparationReceipt, CoreError> {
        let mut receipts = self.receipts.lock().await;
        let operation = intent.operation_id()?;
        if let Some(receipt) = receipts.get(&operation) {
            receipt.matches(intent)?;
            return Ok(receipt.clone());
        }
        let binding = self
            .store
            .binding(&intent.binding_id)
            .await
            .unwrap()
            .unwrap();
        let sandbox = self
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let workspace = self
            .store
            .get_workspace(&binding.workspace_id)
            .await
            .unwrap()
            .unwrap();
        // Use the actual domain transition/receipt validator. This isolated
        // transport fixture does not model Mongo's multi-record transaction.
        let receipt = binding.prepare_retired_hierarchy(
            intent,
            &sandbox,
            &workspace,
            key,
            layer2_core::now_rfc3339(),
        )?;
        receipts.insert(operation, receipt.clone());
        Ok(receipt)
    }
}

pub(in crate::rest) async fn fixture(mode: &str) -> Router {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let service = BindingService::new(
        store.clone(),
        BindingResources {
            sandboxes: SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
            ),
            workspaces: WorkspaceService::new(store.clone(), profiles.clone()),
            agents: AgentService::new(
                store.clone(),
                profiles,
                Arc::new(Registry),
                layer2_core::RuntimeCapabilities::new([
                    layer2_core::RuntimeCapability::Conversation,
                ]),
            ),
        },
        Arc::new(PreparationCustody {
            store: store.clone(),
            receipts: Default::default(),
        }),
        BindingPolicy {
            provider: AgentProvider::default(),
            supported_model_api: "responses".into(),
            bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            sandbox: Arc::new(SandboxPolicy::default()),
        },
    );
    let app = route_service(service, None, None, mode != "disabled").unwrap();
    let mut input = body();
    input["agent_key"] = KEY.into();
    let response = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/v1/cmaeg/bindings/binding-contract",
            Some(input),
            Some(authority()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut binding = store.binding("binding-contract").await.unwrap().unwrap();
    let revision = binding.revision;
    binding.state = if mode == "terminal" {
        BindingState::Retired
    } else {
        BindingState::Ready
    };
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    if mode == "missing" {
        return app;
    }
    let mut sandbox = super::super::agents_tests::sandbox(binding.sandbox_id.as_str(), "owner");
    sandbox.credential.source = binding.credential_source();
    if mode == "corrupt" {
        sandbox.owner = SubjectId::new("other-owner");
    }
    if mode == "retired" {
        sandbox.status = SandboxStatus::Retired;
        sandbox.runtime = None;
        sandbox.retired_at = Some("2026-01-01T00:00:00Z".into());
        let mut home = SandboxHome::reserve(
            &sandbox.id,
            layer3_adapter::HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "storage".into(),
                capacity_gib: 64,
                template_name: "seed".into(),
                template_generation: "original".into(),
            },
        )
        .unwrap();
        home.released = true;
        sandbox.lifecycle.home = Some(Box::new(home));
    }
    store.create_sandbox(sandbox).await.unwrap();
    store
        .create_workspace(super::super::agents_tests::workspace(
            binding.workspace_id.as_str(),
            binding.sandbox_id.as_str(),
            "owner",
        ))
        .await
        .unwrap();
    app
}
