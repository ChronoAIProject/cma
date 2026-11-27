use super::*;

#[tokio::test]
async fn startup_keeps_the_admitted_image_when_the_runtime_default_changes() {
    let new_default = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let fixture =
        NativeStartup::with_default_image(false, false, None, Some(new_default.clone())).await;
    let binding = fixture.original.runtime.as_ref().unwrap();
    assert_ne!(binding.runtime_version, new_default);
    fixture.control.other_gate.add_permits(64);
    fixture.control.status_gate.add_permits(16);

    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.reconciler.initialize_before_workers(fixture.now),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(!fixture.reconciler.binding_is_held(&fixture.original.id));
    fixture.control.assert_siblings_settled();
    let retained = fixture
        .store
        .inner
        .get_sandbox(&fixture.original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained, fixture.original);
    let updates =
        layer2_core::sandbox::SandboxRuntimeUpdateService::new(fixture.store.clone(), new_default);
    assert!(updates.can_update(&retained));
    assert!(retained.runtime_update_request().is_none());
    let state = fixture.native.api.state.lock().unwrap();
    assert_eq!(state.assignments.len(), 1);
    assert_eq!(
        state.rows[api::MEMBER]["spec"]["podTemplate"]["spec"]["containers"][0]["image"],
        binding.runtime_version
    );
}

#[tokio::test]
async fn startup_holds_admitted_spec_drift_without_adopting_or_reclaiming_the_runtime() {
    check_startup_spec_drift("/spec/operatingMode", None).await;
}

#[tokio::test]
async fn startup_holds_admitted_native_vct_memory_request_drift() {
    check_startup_spec_drift(
        "/spec/podTemplate/spec/containers/0/resources/requests/memory",
        Some(json!("4095Mi")),
    )
    .await;
}

async fn check_startup_spec_drift(path: &str, replacement: Option<serde_json::Value>) {
    let new_default = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let fixture = NativeStartup::with_default_image(false, false, None, Some(new_default)).await;
    let (original_member, provider_before) = {
        let mut state = fixture.native.api.state.lock().unwrap();
        let member = state.rows.get_mut(api::MEMBER).unwrap();
        let original = member.clone();
        if let Some(replacement) = replacement {
            let value = member.pointer_mut(path).unwrap();
            assert_ne!(*value, replacement);
            *value = replacement;
        } else {
            assert_eq!(
                member["spec"]
                    .as_object_mut()
                    .unwrap()
                    .remove("operatingMode"),
                Some(json!("Running"))
            );
        }
        (original, member.clone())
    };
    fixture.control.other_gate.add_permits(64);
    fixture.control.status_gate.add_permits(16);

    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.reconciler.initialize_before_workers(fixture.now),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
    fixture.control.assert_siblings_settled();
    fixture.control.assert_no_cleanup();
    assert_eq!(
        fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap(),
        Some(fixture.original.clone())
    );
    {
        let mut state = fixture.native.api.state.lock().unwrap();
        assert_eq!(state.rows[api::MEMBER], provider_before);
        assert!(state.assignments.is_empty());
        state.rows.insert(api::MEMBER.into(), original_member);
    }

    fixture
        .control
        .background_sweep
        .store(true, Ordering::SeqCst);
    fixture.control.other_gate.add_permits(128);
    for _ in 0..8 {
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::time::resume();
        fixture.reconciler.run_due(fixture.now).await.unwrap();
        if !fixture.reconciler.binding_is_held(&fixture.original.id) {
            break;
        }
    }
    assert!(!fixture.reconciler.binding_is_held(&fixture.original.id));
    assert_eq!(
        fixture.native.api.state.lock().unwrap().assignments.len(),
        1
    );
    let reconciled = fixture
        .store
        .inner
        .get_sandbox(&fixture.original.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reconciled.runtime, fixture.original.runtime);
    assert_eq!(reconciled.lifecycle.home, fixture.original.lifecycle.home);
}
