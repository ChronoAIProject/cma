use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::{
    AdapterError, CredentialFrame, InstallRequest, RestoreBootstrapRequest, RestoreRequest,
    RuntimeAdoptionRequest, RuntimeCredential, RuntimeImage, RuntimeProvisionRequest,
    SandboxAdapter, SandboxSpec, WorkspaceRequest, WorkspaceSource, RUNTIME_CONTRACT_LABEL,
    RUNTIME_CONTRACT_VERSION,
};

use super::{HostedDeployment, InMemoryHostedBackend};

fn spec() -> SandboxSpec {
    SandboxSpec {
        sandbox_id: "sbx-recovery".into(),
        ..SandboxSpec::default()
    }
}

fn adapter(backend: InMemoryHostedBackend, image: &str) -> CodexAdapter {
    CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec {
            deployment: HostedDeployment {
                image: RuntimeImage {
                    uri: image.into(),
                    labels: [(
                        RUNTIME_CONTRACT_LABEL.into(),
                        RUNTIME_CONTRACT_VERSION.into(),
                    )]
                    .into(),
                },
                ..HostedDeployment::default()
            },
            ..CodexSandboxSpec::default()
        },
    )
}

#[tokio::test]
async fn generations_coexist_and_a_recreated_adapter_adopts_only_the_exact_running_runtime() {
    let backend = InMemoryHostedBackend::default();
    let old = adapter(backend.clone(), "runtime:old");
    let old_handle = old
        .provision_runtime(&RuntimeProvisionRequest::new(1, spec()))
        .await
        .unwrap();
    let replacement = adapter(backend.clone(), "runtime:new");
    let replacement_handle = replacement
        .provision_runtime(&RuntimeProvisionRequest::new(2, spec()))
        .await
        .unwrap();
    assert_ne!(old_handle, replacement_handle);

    let inventory = replacement.runtime_inventory().await.unwrap();
    assert_eq!(inventory.len(), 2);
    assert_eq!(
        inventory
            .iter()
            .map(|runtime| runtime.generation)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_ne!(inventory[0].image_sha256, inventory[1].image_sha256);

    let recreated = adapter(backend, "runtime:new");
    let mismatch = recreated
        .adopt_runtime(&RuntimeAdoptionRequest::new(2, old_handle.clone(), spec()))
        .await
        .unwrap_err();
    assert!(matches!(mismatch, AdapterError::Rejected(_)));
    assert!(matches!(
        recreated.resources(&old_handle).await,
        Err(AdapterError::Unavailable(_))
    ));

    let missing = recreated
        .adopt_runtime(&RuntimeAdoptionRequest::new(
            2,
            crate::RuntimeHandle::new("runtime-absent"),
            spec(),
        ))
        .await
        .unwrap();
    assert!(missing.is_none());

    let adopted = recreated
        .adopt_runtime(&RuntimeAdoptionRequest::new(
            2,
            replacement_handle.clone(),
            spec(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(adopted.handle, replacement_handle);
    assert_eq!(adopted.generation, 2);
    recreated.resources(&replacement_handle).await.unwrap();
}

#[tokio::test]
async fn restore_bootstrap_replays_exactly_and_fences_work_until_exact_restore() {
    let backend = InMemoryHostedBackend::default();
    let adapter = adapter(backend.clone(), "runtime:new");
    let handle = adapter
        .provision_runtime(&RuntimeProvisionRequest::new(2, spec()))
        .await
        .unwrap();
    let digest = "a".repeat(64);
    let restore = RestoreRequest {
        object_reference: format!("home-{digest}.cma"),
        sha256: digest,
    };
    let bootstrap = RestoreBootstrapRequest::new(
        "sandbox:sbx-recovery:restore-bootstrap:v1",
        "sbx-recovery",
        restore.clone(),
    );
    backend.lose_next_response(crate::wire::WireOperation::BootstrapRestore);
    assert!(matches!(
        adapter.bootstrap_restore(&handle, &bootstrap).await,
        Err(AdapterError::Unavailable(_))
    ));
    let receipt = adapter
        .bootstrap_restore(&handle, &bootstrap)
        .await
        .unwrap();
    assert_eq!(
        adapter
            .bootstrap_restore(&handle, &bootstrap)
            .await
            .unwrap(),
        receipt
    );

    let install = adapter
        .run_install(
            &handle,
            &InstallRequest::new("must-not-run", vec!["false".into()]),
        )
        .await
        .unwrap_err();
    assert!(matches!(install, AdapterError::Rejected(_)));
    adapter
        .deliver_credential(
            &handle,
            CredentialFrame {
                operation_id: "credential-replacement".into(),
                value: RuntimeCredential::new("secret"),
                custody_generation: Some(2),
            },
        )
        .await
        .unwrap();

    let blocked = adapter
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                "workspace:wks-blocked:prepare:v1",
                "wks-blocked",
                "blocked",
                WorkspaceSource::Init,
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(blocked, AdapterError::Rejected(_)));

    let mut changed = restore.clone();
    changed.sha256 = "b".repeat(64);
    changed.object_reference = format!("home-{}.cma", changed.sha256);
    assert!(matches!(
        adapter.restore(&handle, &changed).await,
        Err(AdapterError::Rejected(_))
    ));
    adapter.restore(&handle, &restore).await.unwrap();
}
