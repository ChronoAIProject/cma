use super::*;

fn event() -> BindingPreparationEvent {
    let now = crate::seconds_from_rfc3339(&crate::now_rfc3339()).unwrap() as i64;
    BindingPreparationEvent {
        admission_id: "ordinary-event-one".into(),
        target_id: "consumer-one".into(),
        request_hash: "a".repeat(64),
        owner: SubjectId::new("owner"),
        admitted_at: now - 1,
        expires_at: now + 600,
    }
}

async fn fail_binding(f: &Fixture, mut binding: ProviderBinding) -> ProviderBinding {
    let revision = binding.revision;
    binding.state = BindingState::Failed {
        reason: "original hierarchy unavailable".into(),
    };
    binding.work = BindingWork::Idle;
    binding.advance_revision(crate::now_rfc3339()).unwrap();
    f.store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    binding
}

#[tokio::test]
async fn preparation_inspection_authenticates_current_manager_owner_and_key_without_mutation() {
    let f = fixture();
    let binding = f.ready().await;
    let admitted = event();
    for caller in [
        Caller::new(SubjectId::new("manager"), Authorization::new([])),
        Caller::new(
            SubjectId::new("manager"),
            Authorization::new([Capability::ManageGatewayRecords]),
        ),
        Caller::new(
            SubjectId::new("other-manager"),
            f.caller.authorization.clone(),
        ),
    ] {
        assert!(f
            .service
            .inspect_preparation(&caller, &binding.binding_id, &admitted, &f.key)
            .await
            .is_err());
    }
    let mut wrong_owner = admitted.clone();
    wrong_owner.owner = SubjectId::new("other-owner");
    assert!(matches!(
        f.service
            .inspect_preparation(&f.caller, &binding.binding_id, &wrong_owner, &f.key)
            .await,
        Err(BindingError::NotFound)
    ));
    let wrong_key =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    assert!(matches!(
        f.service
            .inspect_preparation(&f.caller, &binding.binding_id, &admitted, &wrong_key)
            .await,
        Err(BindingError::NotFound)
    ));
    assert!(matches!(
        f.service
            .inspect_preparation(&f.caller, &binding.binding_id, &admitted, &f.key)
            .await
            .unwrap(),
        BindingPreparationInspection::Current { generation: 1, .. }
    ));
    assert_eq!(
        f.store.binding(&binding.binding_id).await.unwrap(),
        Some(binding)
    );
}

#[tokio::test]
async fn preparation_inspection_never_replaces_a_recoverable_original_home() {
    let f = fixture();
    let binding = fail_binding(&f, f.ready().await).await;
    let sandbox = f.store.get_sandbox(&binding.sandbox_id).await.unwrap();
    let workspace = f.store.get_workspace(&binding.workspace_id).await.unwrap();
    for _ in 0..3 {
        assert!(matches!(
            f.service
                .inspect_preparation(&f.caller, &binding.binding_id, &event(), &f.key)
                .await
                .unwrap(),
            BindingPreparationInspection::Current { generation: 1, .. }
        ));
    }
    assert_eq!(
        f.store.binding(&binding.binding_id).await.unwrap(),
        Some(binding.clone())
    );
    assert_eq!(
        f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
        sandbox
    );
    assert_eq!(
        f.store.get_workspace(&binding.workspace_id).await.unwrap(),
        workspace
    );
}

#[tokio::test]
async fn only_a_fresh_event_inspects_a_fully_retired_released_home_as_pending() {
    let f = fixture();
    let binding = fail_binding(&f, f.ready().await).await;
    let mut sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Retired;
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
    sandbox.revision += 1;
    f.store
        .save_sandbox(revision, sandbox.clone())
        .await
        .unwrap();
    let admitted = event();
    let BindingPreparationInspection::Retired { intent } = f
        .service
        .inspect_preparation(&f.caller, &binding.binding_id, &admitted, &f.key)
        .await
        .unwrap()
    else {
        panic!("expected terminal proof");
    };
    assert_eq!(intent.binding_digest, content_digest(&binding).unwrap());
    assert_eq!(intent.sandbox_digest, content_digest(&sandbox).unwrap());
    assert_eq!(intent.generation, binding.generation);
    assert_eq!(
        f.store.binding(&binding.binding_id).await.unwrap(),
        Some(binding.clone())
    );
    // The exact inspection does not confer authority after a manager/key change.
    let mut denied = intent.clone();
    denied.manager = SubjectId::new("other-manager");
    assert!(matches!(
        f.service
            .prepare_retired_binding(&f.caller, &binding.binding_id, &denied, &f.key)
            .await,
        Err(BindingError::NotFound)
    ));
    let mut expired = admitted;
    expired.expires_at = expired.admitted_at;
    assert!(f
        .service
        .inspect_preparation(&f.caller, &binding.binding_id, &expired, &f.key)
        .await
        .is_err());
    assert_eq!(
        f.store.binding(&binding.binding_id).await.unwrap(),
        Some(binding)
    );
}
