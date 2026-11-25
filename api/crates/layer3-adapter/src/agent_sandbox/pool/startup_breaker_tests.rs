use super::*;
use std::time::{Duration, SystemTime};
use tokio::time::Instant;

#[path = "startup_evidence_tests.rs"]
mod evidence;

#[path = "startup_recovery_tests.rs"]
mod recovery;

async fn fixture(enabled: bool) -> Bound {
    bound_config(
        false,
        HomeVolumeMode::NativeVctFullHomeV2,
        None,
        enabled,
        false,
    )
    .await
}

fn restart(bound: &Bound, n: u32, elapsed: i64, exit: i32, reason: &str) {
    let mut state = bound.fixture.state.lock().unwrap();
    let status = &mut state.rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0];
    let prior = status["containerID"].clone();
    let finished = time::OffsetDateTime::from(SystemTime::now()) + time::Duration::seconds(elapsed);
    status["containerID"] = json!(format!("containerd://{n:064x}"));
    status["restartCount"] = json!(n);
    status["lastState"] = json!({"terminated":{
        "containerID":prior,"exitCode":exit,"reason":reason,
        "startedAt":(finished - time::Duration::seconds(2)).format(&time::format_description::well_known::Rfc3339).unwrap(),
        "finishedAt":finished.format(&time::format_description::well_known::Rfc3339).unwrap()
    }});
}

async fn adopt(bound: &Bound) -> Result<Option<RuntimeObservation>> {
    bound.backend.adopt(&bound.runtime, &bound.request).await
}

fn deliveries(bound: &Bound) -> usize {
    bound.fixture.state.lock().unwrap().assignments.len()
}

fn hold(result: Result<Option<RuntimeObservation>>) -> RuntimeStartupFailure {
    let Err(AdapterError::RuntimeStartupHold(failure)) = result else {
        panic!("expected startup hold: {result:?}");
    };
    failure
}

async fn open(bound: &Bound) {
    for n in 1..=3 {
        restart(bound, n, 0, 2, "Error");
        let result = adopt(bound).await;
        if n == 3 {
            assert_eq!(hold(result).consecutive, 3);
        } else {
            result.unwrap().unwrap();
        }
    }
    assert_eq!(deliveries(bound), 3);
}

// Keep the runtime awake during real loopback IO so paused virtual time advances
// only at the explicit probe boundaries, rather than to a transport timeout.
async fn io<T>(future: impl std::future::Future<Output = T>) -> T {
    let awake = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    let result = future.await;
    awake.abort();
    result
}

#[tokio::test]
async fn startup_breaker_disabled_preserves_deliveries_and_never_reads_termination() {
    let bound = fixture(false).await;
    for n in 1..=6 {
        restart(&bound, n, 0, 2, "Error");
        adopt(&bound).await.unwrap().unwrap();
    }
    assert_eq!(deliveries(&bound), 7);
    assert!(!bound
        .fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|(call, _)| call == &format!("GET {POD}")));
}

#[tokio::test]
async fn startup_breaker_opens_only_after_three_fast_acknowledged_failures() {
    let bound = fixture(true).await;
    open(&bound).await;
    let reads = bound
        .fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(call, _)| call == &format!("GET {POD}"))
        .count();
    assert_eq!(reads, 3);
    for _ in 0..5 {
        assert_eq!(hold(adopt(&bound).await).consecutive, 3);
    }
    assert_eq!(deliveries(&bound), 3);
}

#[tokio::test]
async fn startup_breaker_probe_schedule_delivers_once_under_concurrent_callers() {
    let bound = Arc::new(fixture(true).await);
    open(&bound).await;
    tokio::time::pause();
    let mut generation = 3;
    for minutes in [15, 30, 60, 60] {
        tokio::time::advance(Duration::from_secs(minutes * 60 - 1)).await;
        io(async {
            hold(adopt(&bound).await);
        })
        .await;
        tokio::time::advance(Duration::from_secs(1)).await;
        let before = deliveries(&bound);
        io(async {
            let mut calls = Vec::new();
            for _ in 0..12 {
                let bound = bound.clone();
                calls.push(tokio::spawn(async move { adopt(&bound).await }));
            }
            for call in calls {
                match call.await.unwrap() {
                    Ok(Some(_)) | Err(AdapterError::RuntimeStartupHold(_)) => {}
                    other => panic!("unexpected probe result: {other:?}"),
                }
            }
        })
        .await;
        assert_eq!(deliveries(&bound), before + 1);
        generation += 1;
        restart(&bound, generation, 0, 2, "Error");
        io(async {
            hold(adopt(&bound).await);
        })
        .await;
    }
}

#[tokio::test]
async fn startup_breaker_replacement_boot_allows_immediate_delivery() {
    let bound = fixture(true).await;
    open(&bound).await;
    {
        let mut state = bound.fixture.state.lock().unwrap();
        let pod = state.rows.get_mut(POD).unwrap();
        pod["metadata"]["uid"] = json!("replacement-pod");
        pod["status"]["containerStatuses"][0]["restartCount"] = json!(0);
    }
    let started = Instant::now();
    adopt(&bound).await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(deliveries(&bound), 4);
}

#[tokio::test]
async fn startup_breaker_ignores_slow_success_missing_and_unrelated_termination() {
    for variant in ["slow", "zero", "missing", "different"] {
        let bound = fixture(true).await;
        for n in 1..=5 {
            restart(
                &bound,
                n,
                if variant == "slow" { 61 } else { 0 },
                if variant == "zero" { 0 } else { 2 },
                "Error",
            );
            {
                let mut state = bound.fixture.state.lock().unwrap();
                let container =
                    &mut state.rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0];
                if variant == "missing" {
                    container["lastState"] = json!({});
                }
                if variant == "different" {
                    container["lastState"]["terminated"]["containerID"] =
                        json!("containerd://ffffffff");
                }
            }
            adopt(&bound).await.unwrap().unwrap();
        }
        assert_eq!(deliveries(&bound), 6, "{variant}");
    }
}

#[tokio::test]
async fn startup_breaker_hold_skips_readiness_wait_and_stabilizes_unavailable_inspection() {
    let bound = fixture(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    open(&bound).await;
    {
        let mut state = bound.fixture.state.lock().unwrap();
        state.rows.get_mut(MEMBER).unwrap()["status"]["conditions"][0]["status"] = json!("False");
        let pod = state.rows.get_mut(POD).unwrap();
        pod["status"]["conditions"][0]["status"] = json!("False");
        pod["status"]["containerStatuses"][0]["state"] =
            json!({"waiting":{"reason":"CrashLoopBackOff"}});
    }
    let started = Instant::now();
    hold(adopt(&bound).await);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(matches!(
        bound
            .backend
            .inspect_native_runtime(&bound.runtime, &bound.request, &home)
            .await,
        Err(AdapterError::RuntimeStartupHold(_))
    ));
    assert_eq!(deliveries(&bound), 3);
}

#[tokio::test]
async fn startup_breaker_fresh_claims_and_healthy_adoption_have_identical_counts_and_timing() {
    for with_home in [false, true] {
        let mut traces = Vec::new();
        for enabled in [false, true] {
            let started = Instant::now();
            let bound = bound_config(
                false,
                HomeVolumeMode::NativeVctFullHomeV2,
                None,
                enabled,
                with_home,
            )
            .await;
            assert!(started.elapsed() < Duration::from_secs(5));
            assert_eq!(deliveries(&bound), 1);
            for _ in 0..4 {
                adopt(&bound).await.unwrap().unwrap();
            }
            assert_eq!(deliveries(&bound), 1);
            let mut calls: Vec<_> = bound
                .fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .map(|(call, _)| call.clone())
                .collect();
            calls.sort();
            traces.push(calls);
        }
        assert_eq!(traces[0], traces[1]);
    }
}

#[tokio::test]
async fn startup_breaker_nonexecuting_inspection_reset_allows_immediate_delivery() {
    let bound = fixture(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    open(&bound).await;
    let original = {
        let mut state = bound.fixture.state.lock().unwrap();
        let original = state.rows[POD].clone();
        state.rows.get_mut(MEMBER).unwrap()["status"]["conditions"][0]["status"] = json!("False");
        let template = state.rows[MEMBER]["spec"]["podTemplate"]["spec"].clone();
        let pod = state.rows.get_mut(POD).unwrap();
        pod["spec"] = template;
        pod["spec"]["volumes"] = json!([{"name":"home","persistentVolumeClaim":{"claimName":home.receipt.storage_reference}}]);
        pod["metadata"]["resourceVersion"] = json!("72");
        pod["status"]["conditions"][0]["status"] = json!("False");
        pod["status"]["initContainerStatuses"] = json!([{
            "name":"initialize-home","image":"runtime","imageID":"image-id","ready":true,"restartCount":0,
            "state":{"terminated":{"exitCode":0,"finishedAt":"2026-10-28T06:00:00Z"}}
        }]);
        let status = &mut pod["status"]["containerStatuses"][0];
        status["ready"] = json!(false);
        status["started"] = json!(false);
        status["containerID"] = status["lastState"]["terminated"]["containerID"].clone();
        status["state"] = json!({"waiting":{"reason":"CrashLoopBackOff"}});
        original
    };
    bound
        .backend
        .inspect_nonexecuting_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    {
        let mut state = bound.fixture.state.lock().unwrap();
        state.rows.insert(POD.into(), original);
        state.rows.get_mut(MEMBER).unwrap()["status"]["conditions"][0]["status"] = json!("True");
    }
    let started = Instant::now();
    adopt(&bound).await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(deliveries(&bound), 4);
}

struct Serving(Arc<Transport>);
#[async_trait::async_trait]
impl HostedControlTransport for Serving {
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        Ok(())
    }
    async fn bind_runtime(&self, handle: &RuntimeHandle, sandbox: &str) -> Result<()> {
        self.0.bind_runtime(handle, sandbox).await
    }
    async fn exchange(&self, _: &RuntimeHandle, frame: RequestFrame) -> Result<ResponseFrame> {
        let RequestFrame::RuntimeStatus(request) = frame else {
            panic!("only status");
        };
        Ok(ResponseFrame::RuntimeStatus(RuntimeStatus {
            sandbox_id: request.sandbox_id,
            generation: request.generation,
            home_identity: request.home_identity,
            boot_id: "a".repeat(32),
            installation_ready: true,
            credential_ready: true,
            restore_pending: false,
            agent_filesystem: false,
            agent_attachment_upload: false,
            agent_skill_install: true,
            agent_skill_upgrade: false,
            agent_model_catalog: true,
            agent_browser: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
            agent_processes: 0,
        }))
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("no Agent admission");
    }
}

#[tokio::test]
async fn startup_breaker_probe_serving_completion_closes_and_restores_delivery() {
    let mut bound = fixture(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    bound.fixture.state.lock().unwrap().assignments.clear();
    bound.backend = AgentSandboxBackend::new(
        client(&bound.fixture, false),
        Arc::new(Serving(bound.transport.clone())),
    );
    bound
        .backend
        .provision(bound.request.clone())
        .await
        .unwrap();
    open(&bound).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15 * 60)).await;
    io(adopt(&bound)).await.unwrap().unwrap();
    assert_eq!(deliveries(&bound), 4);
    io(bound
        .backend
        .inspect_native_runtime(&bound.runtime, &bound.request, &home))
    .await
    .unwrap()
    .unwrap();
    restart(&bound, 4, 0, 2, "Error");
    io(adopt(&bound)).await.unwrap().unwrap();
    assert_eq!(deliveries(&bound), 5);
}

#[tokio::test]
async fn startup_breaker_ignores_historical_restarts_and_unacknowledged_deliveries() {
    let bound = fixture(true).await;
    restart(&bound, 400, 0, 2, "Error");
    bound.fixture.state.lock().unwrap().fail_exec = true;
    assert!(adopt(&bound).await.is_err());
    for n in 401..405 {
        restart(&bound, n, 0, 2, "Error");
        assert!(adopt(&bound).await.is_err());
    }
    bound.fixture.state.lock().unwrap().fail_exec = false;
    adopt(&bound).await.unwrap().unwrap();
    restart(&bound, 405, 0, 2, "Error");
    adopt(&bound).await.unwrap().unwrap();
    restart(&bound, 406, 0, 2, "Error");
    assert_eq!(hold(adopt(&bound).await).consecutive, 3);
}

#[tokio::test]
async fn startup_breaker_fresh_resume_revive_and_upgrade_deliver_without_delay() {
    for enabled in [false, true] {
        for route in ["resume", "resume_with_home", "upgrade_with_home"] {
            let bound = fixture(enabled).await;
            let home = adopted(&bound.fixture, &bound.claim).await;
            if enabled {
                open(&bound).await;
            }
            let before = deliveries(&bound);
            let mut replacement = bound
                .fixture
                .state
                .lock()
                .unwrap()
                .rows
                .remove(POD)
                .unwrap();
            let provider = client(&bound.fixture, false);
            provider
                .suspend(&bound.runtime, &bound.request)
                .await
                .unwrap();
            let mut next = request(&home, 2);
            if route == "upgrade_with_home" {
                next.image.uri = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
                next.metadata.insert(
                    "cma/image-sha256".into(),
                    crate::hosted::image_sha256(&next.image.uri),
                );
                next.metadata.remove("cma/spec-sha256");
                next.metadata.insert(
                    "cma/spec-sha256".into(),
                    crate::hosted::provider_request_fingerprint(&next).unwrap(),
                );
                provider
                    .upgrade_with_home(&bound.runtime, &bound.request, &next, &home)
                    .await
                    .unwrap();
            } else {
                provider
                    .resume_with_home(&bound.runtime, &bound.request, &next, &home)
                    .await
                    .unwrap();
            }
            {
                let mut state = bound.fixture.state.lock().unwrap();
                replacement["metadata"]["uid"] = json!("fresh-boot");
                replacement["status"]["containerStatuses"][0]["containerID"] =
                    json!(format!("containerd://{}", "b".repeat(64)));
                replacement["status"]["containerStatuses"][0]["restartCount"] = json!(0);
                replacement["status"]["containerStatuses"][0]["lastState"] = json!({});
                state.rows.insert(POD.into(), replacement);
                let member = state.rows.get_mut(MEMBER).unwrap();
                member["status"]["conditions"][0]["observedGeneration"] =
                    member["metadata"]["generation"].clone();
            }
            let started = Instant::now();
            match route {
                "resume" => {
                    bound
                        .backend
                        .resume(&bound.runtime, &bound.request, &next)
                        .await
                }
                "resume_with_home" => {
                    bound
                        .backend
                        .resume_with_home(&bound.runtime, &bound.request, &next, &home)
                        .await
                }
                _ => {
                    bound
                        .backend
                        .upgrade_with_home(&bound.runtime, &bound.request, &next, &home)
                        .await
                }
            }
            .unwrap();
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{route}, enabled={enabled}"
            );
            assert_eq!(deliveries(&bound), before + 1, "{route}, enabled={enabled}");
        }
    }
}

#[tokio::test]
async fn startup_breaker_keeps_concurrent_healthy_deliveries_and_reads_identical_when_closed() {
    let mut traces = Vec::new();
    for enabled in [false, true] {
        let bound = fixture(enabled).await;
        bound.fixture.state.lock().unwrap().calls.clear();
        tokio::time::pause();
        let started = Instant::now();
        io(async {
            let request = &bound.request;
            let (a, b, c, d) = tokio::join!(
                bound.backend.provision(request.clone()),
                bound.backend.provision(request.clone()),
                bound.backend.provision(request.clone()),
                bound.backend.provision(request.clone()),
            );
            for result in [a, b, c, d] {
                assert_eq!(result.unwrap(), bound.runtime);
            }
        })
        .await;
        assert_eq!(started.elapsed(), Duration::ZERO);
        tokio::time::resume();
        assert_eq!(
            deliveries(&bound),
            5,
            "ordinary duplicate deliveries remain unchanged"
        );
        let mut calls: Vec<_> = bound
            .fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|(call, _)| call.clone())
            .collect();
        calls.sort();
        assert!(!calls.contains(&format!("GET {POD}")));
        traces.push(calls);
    }
    // Includes all readiness/authority lists: startup_boot must add no read
    // while disabled or while the retained record has never opened.
    assert_eq!(traces[0], traces[1]);
}
