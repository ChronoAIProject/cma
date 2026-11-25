use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::{
    AdapterError, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest,
    HomeVolumeStore, SandboxAdapter, SandboxSpec,
};

fn attachment() -> HomeVolumeAttachment {
    let request = HomeVolumeRequest {
        scope: None,
        volume_id: "home-fixture".into(),
        sandbox_id: "sandbox-fixture".into(),
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-generation".into(),
        },
    };
    let receipt = HomeVolumeReceipt {
        native_authority: None,
        volume_id: request.volume_id.clone(),
        request_sha256: request.computed_sha256(),
        storage_reference: "home-one".into(),
        storage_generation: "original-disk".into(),
    };
    HomeVolumeAttachment { request, receipt }
}

#[derive(Default)]
struct Storage {
    missing: AtomicBool,
}

#[async_trait::async_trait]
impl HomeVolumeStore for Storage {
    async fn inventory(&self) -> crate::Result<Vec<HomeVolumeAttachment>> {
        Ok(vec![attachment()])
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> crate::Result<HomeVolumeReceipt> {
        panic!("runtime operations may only verify an already allocated home")
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> crate::Result<()> {
        if self.missing.load(Ordering::SeqCst) || home != &attachment() {
            Err(AdapterError::Rejected("home is absent or replaced".into()))
        } else {
            Ok(())
        }
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> crate::Result<()> {
        panic!("runtime teardown must not delete a durable home")
    }
}

fn spec() -> SandboxSpec {
    SandboxSpec {
        sandbox_id: "sandbox-fixture".into(),
        home_volume: Some(Box::new(attachment())),
        ..SandboxSpec::default()
    }
}

#[test]
fn mounted_home_pins_storage_generation_and_keeps_all_writable_state_on_one_disk() {
    let deployment = super::HostedDeployment::default();
    let original = deployment.render(&spec()).unwrap();
    assert_eq!(original.env["TMPDIR"], super::home::TRANSFER_ROOT);
    assert_eq!(
        original.env["CHRONO_RUNTIME_HOME_IDENTITY"],
        attachment().identity().unwrap()
    );
    assert_eq!(original.volumes.len(), 3);
    assert_eq!(
        original
            .volumes
            .iter()
            .map(|mount| (mount.mount_path.as_str(), mount.sub_path.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("/home/cma", Some("home")),
            ("/var/lib/cma-runtime", Some("state")),
            ("/var/lib/cma-transfer", Some("transfer"))
        ]
    );
    assert!(original
        .volumes
        .iter()
        .all(|mount| mount.pvc.claim_name == "home-one"
            && !mount.pvc.create_if_not_exists
            && !mount.pvc.delete_on_sandbox_termination));
    let mut changed = spec();
    changed
        .home_volume
        .as_mut()
        .unwrap()
        .receipt
        .storage_generation = "replacement-disk".into();
    let replacement = deployment.render(&changed).unwrap();
    assert_ne!(
        original.metadata["cma/spec-sha256"],
        replacement.metadata["cma/spec-sha256"]
    );
    assert_eq!(original.volumes, replacement.volumes);
    changed.sandbox_id = "foreign".into();
    assert!(deployment.render(&changed).is_err());
    let mut changed = spec();
    changed.environment.insert("TMPDIR".into(), "/tmp".into());
    assert!(deployment.render(&changed).is_err());
}

#[tokio::test]
async fn absent_storage_support_never_provisions_an_ephemeral_replacement() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    assert!(matches!(
        adapter.provision(&spec()).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(adapter.runtime_inventory().await.unwrap().is_empty());
}

#[tokio::test]
async fn reattachment_and_credentials_require_the_original_disk_and_teardown_preserves_it() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let store = Arc::new(Storage::default());
    let adapter = adapter.with_home_volumes(store.clone());
    let spec = spec();
    let handle = adapter.provision(&spec).await.unwrap();
    let restarted = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default())
        .with_home_volumes(store.clone());
    let request = crate::RuntimeAdoptionRequest::new(1, handle.clone(), spec);
    restarted.adopt_runtime(&request).await.unwrap().unwrap();
    store.missing.store(true, Ordering::SeqCst);
    assert!(restarted.adopt_runtime(&request).await.is_err());
    assert!(restarted
        .run_install(&handle, &crate::InstallRequest::new("install", vec![]))
        .await
        .is_err());
    assert!(restarted
        .deliver_credential(
            &handle,
            crate::CredentialFrame {
                operation_id: "credential-fixture".into(),
                value: crate::RuntimeCredential::new("opaque-sandbox-key"),
                custody_generation: Some(1),
                github_route: None,
            }
        )
        .await
        .is_err());
    assert!(backend.requests().await.is_empty());
    assert!(
        !backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
    store.missing.store(false, Ordering::SeqCst);
    restarted.teardown(&handle).await.unwrap();
    assert!(restarted.runtime_inventory().await.unwrap().is_empty());
    assert_eq!(store.inventory().await.unwrap(), [attachment()]);
}

#[tokio::test]
async fn runtime_status_requires_exact_persistent_storage_and_does_not_deliver_credentials() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let store = Arc::new(Storage::default());
    let adapter = adapter.with_home_volumes(store.clone());
    let handle = adapter.provision(&spec()).await.unwrap();
    let initial = adapter.runtime_status(&handle).await.unwrap();
    assert!(!initial.installation_ready && !initial.credential_ready);
    assert_eq!(initial.home_identity, attachment().identity().unwrap());
    adapter
        .run_install(&handle, &crate::InstallRequest::new("install", vec![]))
        .await
        .unwrap();
    let installed = adapter.runtime_status(&handle).await.unwrap();
    assert_eq!(installed.boot_id, initial.boot_id);
    assert!(installed.installation_ready && !installed.credential_ready);
    let before = backend.requests().await.len();
    store.missing.store(true, Ordering::SeqCst);
    assert!(adapter.runtime_status(&handle).await.is_err());
    assert_eq!(backend.requests().await.len(), before);
    assert!(
        !backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
}

#[tokio::test]
async fn retained_v3_runtime_without_live_removal_protocol_cannot_claim_capability() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let adapter = adapter.with_home_volumes(Arc::new(Storage::default()));
    let handle = adapter.provision(&spec()).await.unwrap();
    assert!(adapter.capabilities().conversation);
    assert!(matches!(
        adapter.workspace_removal_support(&handle).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(
        !backend
            .snapshot(&handle)
            .await
            .unwrap()
            .credential_delivered
    );
    assert!(backend.requests().await.iter().all(|(_, request)| matches!(
        request,
        crate::wire::RequestFrame::RuntimeStatus(_)
            | crate::wire::RequestFrame::WorkspaceRemovalSupport(_)
    )));
}

#[tokio::test]
async fn retained_runtime_without_github_route_support_receives_no_new_credential() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let adapter = adapter.with_home_volumes(Arc::new(Storage::default()));
    let handle = adapter.provision(&spec()).await.unwrap();
    adapter
        .run_install(&handle, &crate::InstallRequest::new("install", vec![]))
        .await
        .unwrap();
    let frame = crate::CredentialFrame {
        operation_id: "exact-github-route".into(),
        value: crate::RuntimeCredential::new("synthetic-credential"),
        custody_generation: Some(1),
        github_route: Some(crate::GithubCredentialRoute {
            service_id: "repository-connection".into(),
            catalog_id: "github-catalog".into(),
            repository_scope: None,
            other_services: Vec::new(),
        }),
    };
    assert!(matches!(
        adapter.deliver_credential(&handle, frame).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(backend.requests().await.iter().all(|(_, request)| {
        !matches!(
            request,
            crate::wire::RequestFrame::Credential(_)
                | crate::wire::RequestFrame::CredentialForReceiver { .. }
        )
    }));
}

#[path = "resume_tests.rs"]
mod resume;
