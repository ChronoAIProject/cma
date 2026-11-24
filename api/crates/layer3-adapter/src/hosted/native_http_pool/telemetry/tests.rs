use super::*;
use native_pool::telemetry::{Event, Gauge, Observer, Outcome, Pool, Refusal};

#[derive(Default)]
struct Probe(StdMutex<Vec<Event>>);
impl Observer for Probe {
    fn record(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}
impl Probe {
    fn gauge(&self, kind: Gauge) -> i64 {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::Gauge(observed, delta) if *observed == kind => Some(delta),
                _ => None,
            })
            .sum()
    }
    fn checkouts(&self, outcome: Outcome) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(event,
            Event::Checkout(_, observed, _) if *observed == outcome)
            })
            .count()
    }
    fn refusals(&self, reason: Refusal) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(event,
            Event::Refused(_, _, observed) if *observed == reason)
            })
            .count()
    }
    fn occupancy(&self, slots: i64, cached: i64, busy: i64, waiting: i64) {
        let mut observed = [0i64; 7];
        for event in self.0.lock().unwrap().iter() {
            if let Event::Gauge(kind, delta) = event {
                observed[*kind as usize] += delta;
                assert!(observed[*kind as usize] >= 0, "negative {kind:?}");
                assert!(
                    observed[Gauge::Busy as usize] + observed[Gauge::CachedConnections as usize]
                        <= observed[Gauge::Slots as usize],
                    "a session was counted both busy and cached"
                );
            }
        }
        for (kind, expected) in [
            (Gauge::Slots, slots),
            (Gauge::CachedConnections, cached),
            (Gauge::Busy, busy),
            (Gauge::Waiting, waiting),
        ] {
            assert_eq!(self.gauge(kind), expected, "{kind:?}");
        }
    }
}

#[tokio::test]
async fn retired_idle_slot_remains_counted_until_its_last_handle_drops() {
    let (fixture, probe, endpoint) = fixture(0).await;
    request(&fixture, &endpoint).await;
    let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
    fixture.transport.retire(&handle()).await.unwrap();
    probe.occupancy(1, 1, 0, 0);
    drop(slot);
    probe.occupancy(0, 0, 0, 0);
    fixture.drained().await;
}
async fn instrument(fixture: &Fixture) -> Arc<Probe> {
    let probe = Arc::new(Probe::default());
    let mut state = fixture.transport.state.lock().await;
    assert!(state.native_http.is_empty());
    state.native_http_telemetry = Some(Pool::new(
        probe.clone(),
        fixture
            .transport
            .runtime_capacity
            .config()
            .native_http_session_reserve,
    ));
    probe
}
async fn fixture(reserve: usize) -> (Fixture, Arc<Probe>, RuntimeEndpoint) {
    let fixture = Fixture::with_capacity(
        Mode::Persistent,
        NativeRuntimeTransport::KubernetesExec,
        crate::agent_sandbox::RuntimeCapacityConfig {
            native_http_session_reserve: reserve,
            ..Default::default()
        },
    )
    .await;
    let probe = instrument(&fixture).await;
    fixture
        .transport
        .bind_native_runtime(&handle(), "sandbox-measured", &"a".repeat(64))
        .await
        .unwrap();
    let endpoint = fixture.endpoint().await;
    (fixture, probe, endpoint)
}
async fn request(fixture: &Fixture, endpoint: &RuntimeEndpoint) {
    fixture
        .transport
        .control(endpoint, &RequestFrame::Resources)
        .await
        .unwrap();
}

#[tokio::test]
async fn clones_reuse_one_pool_and_drop_releases_all_gauges() {
    let (fixture, probe, endpoint) = fixture(2).await;
    request(&fixture, &endpoint).await;
    let clone = fixture.transport.as_ref().clone();
    clone
        .control(&endpoint, &RequestFrame::Resources)
        .await
        .unwrap();
    probe.occupancy(1, 1, 0, 0);
    assert_eq!(probe.checkouts(Outcome::Fresh), 1);
    assert_eq!(probe.checkouts(Outcome::Reused), 1);
    assert_eq!(probe.gauge(Gauge::Pools), 1);
    assert_eq!(probe.gauge(Gauge::Capacity), 16);
    assert_eq!(probe.gauge(Gauge::Reserve), 2);
    drop(clone);
    assert_eq!(probe.gauge(Gauge::Pools), 1);
    drop(fixture);
    probe.occupancy(0, 0, 0, 0);
    assert_eq!(probe.gauge(Gauge::Pools), 0);
    assert_eq!(probe.gauge(Gauge::Capacity), 0);
    assert_eq!(probe.gauge(Gauge::Reserve), 0);
}

#[tokio::test]
async fn waiting_cancellation_and_post_checkout_drop_balance_separately() {
    let (fixture, probe, endpoint) = fixture(0).await;
    request(&fixture, &endpoint).await;
    let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
    let lease = fixture
        .transport
        .native_checkout(&endpoint, slot.clone())
        .await
        .unwrap();
    probe.occupancy(1, 0, 1, 0);
    let mut waiting = Box::pin(fixture.transport.native_checkout(&endpoint, slot.clone()));
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    probe.occupancy(1, 0, 1, 1);
    drop(waiting);
    probe.occupancy(1, 0, 1, 0);
    assert_eq!(probe.checkouts(Outcome::Cancelled), 1);
    drop(lease);
    probe.occupancy(1, 0, 0, 0);
    drop(slot);
    request(&fixture, &endpoint).await;
    assert_eq!(fixture.state.opened.load(Ordering::SeqCst), 2);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    probe.occupancy(0, 0, 0, 0);
}

#[tokio::test]
async fn retired_queued_checkout_sends_no_request_and_balances_on_drop() {
    let (fixture, probe, endpoint) = fixture(0).await;
    request(&fixture, &endpoint).await;
    let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
    let lease = fixture
        .transport
        .native_checkout(&endpoint, slot.clone())
        .await
        .unwrap();
    let mut waiting = Box::pin(fixture.transport.native_checkout(&endpoint, slot.clone()));
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    fixture.transport.retire(&handle()).await.unwrap();
    drop(lease);
    assert!(waiting.await.is_err());
    assert_eq!(probe.checkouts(Outcome::Retired), 1);
    drop(slot);
    probe.occupancy(0, 0, 0, 0);
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    fixture.drained().await;
}

#[tokio::test]
async fn response_cancellation_releases_busy_session_and_request_permit() {
    let (fixture, probe, endpoint) = fixture(0).await;
    fixture.state.hold_response.store(true, Ordering::SeqCst);
    let mut pending = Box::pin(
        fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources),
    );
    tokio::select! {
        result = &mut pending => panic!("response unexpectedly completed: {result:?}"),
        _ = async {
            while fixture.state.requests.lock().unwrap().is_empty() { tokio::task::yield_now().await; }
        } => {},
        _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("request did not arrive"),
    }
    probe.occupancy(1, 0, 1, 0);
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 31);
    drop(pending);
    probe.occupancy(1, 0, 0, 0);
    assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    probe.occupancy(0, 0, 0, 0);
}

#[tokio::test]
async fn protocol_errors_release_checkout_without_caching_broken_connections() {
    for mode in [
        Mode::HttpError,
        Mode::LostResponse,
        Mode::Forbidden,
        Mode::ErrorChannel,
    ] {
        let fixture = Fixture::start(mode, NativeRuntimeTransport::KubernetesExec).await;
        let probe = instrument(&fixture).await;
        let endpoint = fixture.endpoint().await;
        assert!(fixture
            .transport
            .control(&endpoint, &RequestFrame::Resources)
            .await
            .is_err());
        assert_eq!(probe.gauge(Gauge::CachedConnections), 0);
        assert_eq!(probe.gauge(Gauge::Busy), 0);
        assert_eq!(probe.gauge(Gauge::Waiting), 0);
        assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
        probe.occupancy(0, 0, 0, 0);
    }
}

#[tokio::test]
async fn wire_failure_and_oversized_body_preserve_results_and_release_checkout() {
    for mode in [Mode::WireError, Mode::LargeBody] {
        let fixture = Fixture::start(mode, NativeRuntimeTransport::KubernetesExec).await;
        let probe = instrument(&fixture).await;
        let endpoint = fixture.endpoint().await;
        if matches!(mode, Mode::WireError) {
            assert!(matches!(
                fixture
                    .transport
                    .control(&endpoint, &RequestFrame::Resources)
                    .await
                    .unwrap(),
                ResponseFrame::Failure(_)
            ));
        } else {
            assert!(fixture
                .transport
                .native_http(
                    &endpoint,
                    http::Method::POST,
                    endpoint.control().unwrap(),
                    Vec::new(),
                    32
                )
                .await
                .is_err());
        }
        assert_eq!(probe.gauge(Gauge::Busy), 0);
        assert_eq!(probe.gauge(Gauge::Waiting), 0);
        assert_eq!(fixture.transport.runtime_capacity.available_requests(), 32);
        fixture.transport.retire(&handle()).await.unwrap();
        fixture.drained().await;
        probe.occupancy(0, 0, 0, 0);
    }
}

#[tokio::test]
async fn incarnation_replacement_releases_cache_before_any_stale_send() {
    let (fixture, probe, endpoint) = fixture(0).await;
    request(&fixture, &endpoint).await;
    fixture.state.restarted.store(true, Ordering::SeqCst);
    assert!(fixture
        .transport
        .control(&endpoint, &RequestFrame::Resources)
        .await
        .is_err());
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    probe.occupancy(0, 0, 0, 0);
    let replacement = fixture.endpoint().await;
    request(&fixture, &replacement).await;
    probe.occupancy(1, 1, 0, 0);
    fixture.transport.retire(&handle()).await.unwrap();
    fixture.drained().await;
    probe.occupancy(0, 0, 0, 0);
}

#[tokio::test]
async fn idle_timer_removes_cache_but_never_an_active_lease() {
    let (fixture, probe, endpoint) = fixture(0).await;
    request(&fixture, &endpoint).await;
    let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
    let lease = fixture
        .transport
        .native_checkout(&endpoint, slot.clone())
        .await
        .unwrap();
    lease.recycle_with_idle_ttl(Duration::ZERO).await.unwrap();
    let lease = fixture
        .transport
        .native_checkout(&endpoint, slot.clone())
        .await
        .unwrap();
    tokio::task::yield_now().await;
    probe.occupancy(1, 0, 1, 0);
    lease.recycle_with_idle_ttl(Duration::ZERO).await.unwrap();
    drop(slot);
    tokio::time::timeout(Duration::from_secs(2), async {
        while probe.gauge(Gauge::Slots) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    probe.occupancy(0, 0, 0, 0);
    fixture.drained().await;
}

#[tokio::test]
async fn reserve_and_physical_exhaustion_are_distinct_and_idle_eviction_balances() {
    for reserve in [0, 1] {
        let (fixture, probe, _) = fixture(reserve).await;
        let RuntimeDiscovery::AgentSandbox(client) = &fixture.transport.client else {
            unreachable!()
        };
        let mut leases = Vec::new();
        let mut handles = Vec::new();
        for index in 0..=16 {
            let name = format!("measured-{index}");
            let handle = RuntimeHandle::new(format!("agent-sandbox:{name}:{}", sandbox_uid(&name)));
            fixture
                .state
                .runtimes
                .lock()
                .unwrap()
                .insert(name.clone(), client.control_token(&name));
            fixture
                .transport
                .bind_native_runtime(&handle, &format!("sandbox-{index}"), &"a".repeat(64))
                .await
                .unwrap();
            let endpoint = fixture.transport.endpoint(&handle).await.unwrap();
            if index < 16 - reserve {
                let slot = fixture.transport.native_slot(&endpoint).await.unwrap();
                leases.push(
                    fixture
                        .transport
                        .native_checkout(&endpoint, slot)
                        .await
                        .unwrap(),
                );
            } else {
                assert!(fixture.transport.native_slot(&endpoint).await.is_err());
                let reason = if reserve == 0 {
                    Refusal::Exhausted
                } else {
                    Refusal::Reserve
                };
                assert!(probe.refusals(reason) > 0);
                // Free one entry; a new slot can replace only the idle entry.
                leases.pop();
                request(&fixture, &endpoint).await;
                assert_eq!(probe.gauge(Gauge::Busy), leases.len() as i64);
                break;
            }
            handles.push(handle);
        }
        drop(leases);
        for handle in handles {
            fixture.transport.retire(&handle).await.unwrap();
        }
        drop(fixture);
        probe.occupancy(0, 0, 0, 0);
    }
}

#[tokio::test]
async fn independent_pools_add_metrics_without_overwriting_each_other() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let metrics = recorder.handle();
    let _recorder = metrics::set_default_local_recorder(&recorder);
    let first = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    let second = Fixture::start(Mode::Persistent, NativeRuntimeTransport::KubernetesExec).await;
    for fixture in [&first, &second] {
        let endpoint = fixture.endpoint().await;
        request(fixture, &endpoint).await;
    }
    let metric = "cma_adapter_native_http_pool{pool=\"native\",kind=\"slots\"}";
    assert!(metrics.render().contains(&format!("{metric} 2")));
    drop(first);
    assert!(metrics.render().contains(&format!("{metric} 1")));
    drop(second);
    assert!(metrics.render().contains(&format!("{metric} 0")));
}
