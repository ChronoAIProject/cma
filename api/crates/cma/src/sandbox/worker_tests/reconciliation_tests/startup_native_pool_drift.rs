use super::*;

#[tokio::test]
async fn startup_native_pool_drift_holds_original_binding_and_recovers_after_repair() {
    let fixture = NativeStartup::with_fs_group_policy(
        false,
        false,
        Some(NativeFsGroupChangePolicy::OnRootMismatch),
    )
    .await;
    let binding = &fixture
        .original
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .native_pool
        .as_ref()
        .unwrap()
        .request
        .pool;
    let path = format!("{}/sandboxtemplates/{}", api::PREFIX, binding.template_name);
    let original = {
        let mut api = fixture.native.api.state.lock().unwrap();
        let template = api.rows.get_mut(&path).unwrap();
        let original = template.clone();
        template["spec"]["podTemplate"]["spec"]["securityContext"]["fsGroupChangePolicy"] =
            json!("Always");
        original
    };
    // With both failures present, the member ownership refusal remains fatal.
    let original_member = {
        let mut api = fixture.native.api.state.lock().unwrap();
        let member = api.rows.get_mut(api::MEMBER).unwrap();
        let original_member = member.clone();
        member["metadata"]["ownerReferences"][0]["uid"] = json!("different-claim");
        original_member
    };
    fixture.control.other_gate.add_permits(64);
    fixture.control.status_gate.add_permits(16);
    let failed = tokio::time::timeout(
        Duration::from_secs(10),
        fixture.reconciler.initialize_before_workers(fixture.now),
    )
    .await
    .unwrap();
    assert!(failed.is_err());
    fixture.control.assert_no_cleanup();
    assert!(fixture
        .native
        .api
        .state
        .lock()
        .unwrap()
        .assignments
        .is_empty());
    fixture
        .native
        .api
        .state
        .lock()
        .unwrap()
        .rows
        .insert(api::MEMBER.into(), original_member);
    // A new manager sees the original durable binding with spec drift alone.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::time::resume();
    fixture.control.other_settled.store(0, Ordering::SeqCst);
    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.reconciler.initialize_before_workers(fixture.now),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
    fixture.control.assert_no_cleanup();
    assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
    assert_eq!(
        fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap(),
        Some(fixture.original.clone())
    );
    assert!(fixture
        .native
        .api
        .state
        .lock()
        .unwrap()
        .assignments
        .is_empty());

    fixture
        .control
        .background_sweep
        .store(true, Ordering::SeqCst);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::time::resume();
    fixture.reconciler.run_due(fixture.now).await.unwrap();
    assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
    assert!(fixture.control.other_settled.load(Ordering::SeqCst) >= 16);
    {
        let mut api = fixture.native.api.state.lock().unwrap();
        assert!(api.assignments.is_empty());
        assert!(!api
            .calls
            .iter()
            .any(|(kind, id)| { kind == "ORPHAN_PHASE" && id == fixture.original.id.as_str() }));
        assert!(!api.calls.iter().any(|(method, resource)| {
            method == "DELETE"
                && [fixture.native.claim_path.as_str(), api::MEMBER, api::VOLUME]
                    .contains(&resource.as_str())
        }));
        api.rows.insert(path, original);
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::time::resume();
    for _ in 0..8 {
        fixture.reconciler.run_due(fixture.now).await.unwrap();
        if !fixture.reconciler.binding_is_held(&fixture.original.id) {
            break;
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::time::resume();
    }
    assert!(!fixture.reconciler.binding_is_held(&fixture.original.id));
    assert_eq!(
        fixture
            .store
            .inner
            .get_sandbox(&fixture.original.id)
            .await
            .unwrap(),
        Some(fixture.original.clone())
    );
    assert_eq!(
        fixture.native.api.state.lock().unwrap().assignments.len(),
        1
    );
}

#[tokio::test]
async fn startup_native_pool_drift_or_absence_preserves_retained_binding() {
    for (pool_absent, template_absent) in [(true, false), (false, true), (true, true)] {
        let fixture = NativeStartup::with_fs_group_policy(
            false,
            false,
            Some(NativeFsGroupChangePolicy::OnRootMismatch),
        )
        .await;
        let binding = &fixture
            .original
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .native_pool
            .as_ref()
            .unwrap()
            .request
            .pool;
        let pool = format!("{}/sandboxwarmpools/{}", api::PREFIX, binding.name);
        let template = format!("{}/sandboxtemplates/{}", api::PREFIX, binding.template_name);
        let hidden = {
            let mut state = fixture.native.api.state.lock().unwrap();
            let mut hidden = Vec::new();
            for (absent, path) in [(pool_absent, pool), (template_absent, template)] {
                if absent {
                    let original = state.rows.remove(&path).unwrap();
                    hidden.push((path, original));
                }
            }
            hidden
        };
        if template_absent && !pool_absent {
            let provider = AgentSandboxClient::new(
                fixture.native.api.client.clone(),
                AgentSandboxConfig {
                    namespace: "cma-test".into(),
                    deployment: "test".into(),
                    runtime_class: None,
                    service_account: "cma-runtime".into(),
                    image_pull_secrets: Vec::new(),
                    template_network_policy: None,
                    template_fs_group_change_policy: Some(
                        NativeFsGroupChangePolicy::OnRootMismatch,
                    ),
                    native_claim_guard_enabled: false,
                    runtime_startup_breaker_enabled: false,
                    control_key: "fixture".repeat(8),
                },
                "fixture",
            )
            .unwrap();
            let pools = provider.warm_pools();
            let mut spec = binding.spec.clone();
            spec.cpu_millis += 100;
            let healthy = pools.configure(&spec, 2).await.unwrap();
            {
                let mut state = fixture.native.api.state.lock().unwrap();
                let path = format!("{}/sandboxwarmpools/{}", api::PREFIX, healthy.binding.name);
                let row = state.rows.get_mut(&path).unwrap();
                row["status"] = json!({
                    "observedGeneration": row["metadata"]["generation"],
                    "readyReplicas": 2
                });
            }
            assert_eq!(
                pools.inventory().await.unwrap_err(),
                layer3_adapter::AdapterError::NativePoolDefinitionAbsent {
                    pool: false,
                    template: true,
                }
            );
            crate::warm_pools::tests::startup_tests::assert_startup_pool_with_historical_template_absent(
                pools, pools.plan(&spec).unwrap(), healthy,
            ).await;
        }
        fixture.control.other_gate.add_permits(64);
        fixture.control.status_gate.add_permits(16);
        tokio::time::timeout(
            Duration::from_secs(10),
            fixture.reconciler.initialize_before_workers(fixture.now),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fixture.control.other_settled.load(Ordering::SeqCst), 8);
        fixture.control.assert_no_cleanup();
        assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
        assert_eq!(
            fixture
                .store
                .inner
                .get_sandbox(&fixture.original.id)
                .await
                .unwrap(),
            Some(fixture.original.clone())
        );

        fixture
            .control
            .background_sweep
            .store(true, Ordering::SeqCst);
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::time::resume();
        fixture.reconciler.run_due(fixture.now).await.unwrap();
        assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
        assert!(fixture.control.other_settled.load(Ordering::SeqCst) >= 16);
        {
            let mut state = fixture.native.api.state.lock().unwrap();
            assert!(state.assignments.is_empty());
            assert!(!state.calls.iter().any(|(kind, id)| {
                kind == "ORPHAN_PHASE" && id == fixture.original.id.as_str()
            }));
            assert!(!state.calls.iter().any(|(method, resource)| {
                method == "DELETE"
                    && [fixture.native.claim_path.as_str(), api::MEMBER, api::VOLUME]
                        .contains(&resource.as_str())
            }));
            // Model a replacement seen under the original name. It cannot
            // discharge a hold, even if all its other fields match.
            for (path, object) in &hidden {
                let mut replacement = object.clone();
                replacement["metadata"]["uid"] = json!("replacement");
                state.rows.insert(path.clone(), replacement);
            }
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(60)).await;
        tokio::time::resume();
        fixture.reconciler.run_due(fixture.now).await.unwrap();
        assert!(fixture.reconciler.binding_is_held(&fixture.original.id));
        // Restore provider visibility of the original objects, not recreation.
        {
            let mut state = fixture.native.api.state.lock().unwrap();
            assert!(state.assignments.is_empty());
            for (path, object) in hidden {
                state.rows.insert(path, object);
            }
        }
        for _ in 0..8 {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(300)).await;
            tokio::time::resume();
            fixture.reconciler.run_due(fixture.now).await.unwrap();
            if !fixture.reconciler.binding_is_held(&fixture.original.id) {
                break;
            }
        }
        assert!(!fixture.reconciler.binding_is_held(&fixture.original.id));
        assert_eq!(
            fixture
                .store
                .inner
                .get_sandbox(&fixture.original.id)
                .await
                .unwrap(),
            Some(fixture.original.clone())
        );
        assert_eq!(
            fixture.native.api.state.lock().unwrap().assignments.len(),
            1
        );
    }
}
