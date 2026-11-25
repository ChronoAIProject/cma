use super::*;

pub(super) async fn retirement_case6_negative_boundaries_retain_debt() {
    for case in [
        "provider_denial",
        "provider_timeout",
        "replaced_boot",
        "missing_home",
        "live_children_without_workers",
    ] {
        let (f, active, children, mut now) = active_siblings().await;
        let owners = Owners::new().await;
        let receiver = Receiver::new(&f, &active).await;
        admit(&f, &active).await;
        let clock = Arc::new(Mutex::new(now));
        let worker = finalizing_parent_worker(&f, clock.clone());
        // Produce a real failed close before injecting the independent provider
        // boundary. The no-worker control deliberately never produces one.
        for _ in 0..2 {
            now += Duration::from_secs(6);
            *clock.lock().unwrap() = now + Duration::from_secs(1);
            worker.run_one(&active.id, now).await.unwrap();
            if case != "live_children_without_workers" {
                for (index, owner) in owners.coordinators.iter().enumerate() {
                    let child_worker =
                        child_worker(&f, Some(owner.clone()), &format!("negative-{index}"));
                    for child in &children {
                        let _ = child_worker.run_one(&child.id, now).await;
                    }
                }
            }
        }
        match case {
            "provider_denial" => f.backend.deny_inventory.store(true, Ordering::SeqCst),
            "provider_timeout" => f.backend.timeout_inventory.store(true, Ordering::SeqCst),
            "replaced_boot" => f.backend.restart_process().await,
            "missing_home" => f.homes.lose_disk(),
            "live_children_without_workers" => (),
            _ => unreachable!(),
        }
        for _ in 0..3 {
            now += Duration::from_secs(6);
            *clock.lock().unwrap() = now + Duration::from_secs(1);
            worker.run_one(&active.id, now).await.unwrap();
        }
        let parent = f.harness.stored(&active.id).await;
        assert_eq!(parent.status, SandboxStatus::Retiring, "{case}");
        assert_eq!(parent.work.step, SandboxWorkStep::StopAgents, "{case}");
        assert_eq!(parent.runtime, active.runtime, "{case}");
        assert_eq!(parent.lifecycle.home, active.lifecycle.home, "{case}");
        assert_eq!(parent.credential, active.credential, "{case}");
        assert!(
            !f.backend
                .calls
                .snapshot()
                .iter()
                .any(|c| c.starts_with("delete")),
            "{case}"
        );
        assert!(
            !f.harness
                .actions
                .snapshot()
                .iter()
                .any(|c| c.starts_with("revoke")),
            "{case}"
        );
        for child in &children {
            let child = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
            assert!(child.status.is_live(), "{case}");
        }
        report(
            case,
            &f,
            &active.id,
            &children,
            json!({"complete":false,"attempts":3}),
        )
        .await;
        receiver.server.abort();
        owners.cleanup().await;
    }
}

pub(super) async fn retirement_case6_retained_provider_denial_blocks_paused_siblings() {
    let (f, active, children, mut now) = active_siblings().await;
    let paused = f.pause(&active, &mut now).await;
    assert!(paused.lifecycle.suspended_runtime.is_some());
    f.backend.deny_teardown.store(true, Ordering::SeqCst);
    admit(&f, &paused).await;
    let worker = parent_worker(&f);
    for _ in 0..3 {
        now += Duration::from_secs(6);
        worker.run_one(&paused.id, now).await.unwrap();
    }
    let parent = f.harness.stored(&paused.id).await;
    assert_eq!(parent.status, SandboxStatus::Retiring);
    assert_eq!(parent.work.step, SandboxWorkStep::StopAgents);
    assert_eq!(
        parent.lifecycle.suspended_runtime,
        paused.lifecycle.suspended_runtime
    );
    assert_eq!(parent.lifecycle.home, paused.lifecycle.home);
    assert!(!f
        .backend
        .calls
        .snapshot()
        .iter()
        .any(|c| c == "delete-effective"));
    report(
        "retained_provider_teardown_unavailable",
        &f,
        &paused.id,
        &children,
        json!({"complete":false,"attempts":3}),
    )
    .await;
}

pub(super) async fn retirement_case6_replaced_provider_uid_blocks_retained_delete() {
    use layer3_adapter::agent_sandbox::{AgentSandboxClient, AgentSandboxConfig};
    let (f, active, children, mut now) = active_siblings().await;
    let paused = f.pause(&active, &mut now).await;
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let router=axum::Router::new().fallback(move |method:axum::http::Method| {
        let count=count.clone();
        async move {
            if method != axum::http::Method::GET {count.fetch_add(1,Ordering::SeqCst);}
            axum::Json(json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                "metadata":{"name":"retained-crd","namespace":"retirement-local","uid":"replacement-uid","resourceVersion":"22", "labels":{"cma/control-plane":"retirement-local"}}}))
        }
    });
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new(address.parse().unwrap())).unwrap(),
        AgentSandboxConfig {
            namespace: "retirement-local".into(),
            deployment: "retirement-local".into(),
            runtime_class: None,
            service_account: "default".into(),
            image_pull_secrets: vec![],
            template_network_policy: None,
            template_fs_group_change_policy: None,
            native_claim_guard_enabled: false,
            runtime_startup_breaker_enabled: false,
            control_key: uuid::Uuid::new_v4().simple().to_string(),
        },
        "local-fixture",
    )
    .unwrap();
    *f.backend.retirement_client.lock().unwrap() = Some(Arc::new(client));
    admit(&f, &paused).await;
    let worker = parent_worker(&f);
    for _ in 0..3 {
        now += Duration::from_secs(6);
        worker.run_one(&paused.id, now).await.unwrap();
    }
    let parent = f.harness.stored(&paused.id).await;
    assert_eq!(parent.status, SandboxStatus::Retiring);
    assert_eq!(parent.work.step, SandboxWorkStep::StopAgents);
    assert_eq!(
        parent.lifecycle.suspended_runtime,
        paused.lifecycle.suspended_runtime
    );
    assert_eq!(parent.lifecycle.home, paused.lifecycle.home);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert!(!f
        .backend
        .calls
        .snapshot()
        .iter()
        .any(|c| c == "delete-effective"));
    report(
        "replaced_provider_uid",
        &f,
        &paused.id,
        &children,
        json!({"complete":false,"provider_writes":0,"attempts":3,"client":"AgentSandboxClient"}),
    )
    .await;
    server.abort();
}
