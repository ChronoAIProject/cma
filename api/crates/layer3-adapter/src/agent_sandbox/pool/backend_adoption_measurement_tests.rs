use super::*;
use std::time::Instant;

#[tokio::test]
#[ignore = "explicit component measurement with 20 ms delayed Kubernetes GETs"]
async fn actual_adoption_delayed_reads_compare_discovery_with_persisted_home() {
    for mode in [
        HomeVolumeMode::NativeVctEmptyV1,
        HomeVolumeMode::NativeVctFullHomeV2,
    ] {
        let bound = bound_with_mode(true, mode).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        let delays = bound
            .fixture
            .state
            .lock()
            .unwrap()
            .rows
            .keys()
            .map(|p| (p.clone(), Duration::from_millis(20)))
            .chain(std::iter::once((
                "/api/v1/namespaces/cma-test/pods".into(),
                Duration::from_millis(20),
            )))
            .collect();
        let effects = Effects::with_delays(&bound.fixture, BTreeMap::new(), delays);
        for restart in [false, true] {
            for persisted in [false, true] {
                let make_backend = || {
                    AgentSandboxBackend::new(
                        AgentSandboxClient::new(
                            effects.client.clone(),
                            bound.fixture.client.config.clone(),
                            "fixture",
                        )
                        .unwrap(),
                        bound.transport.clone(),
                    )
                };
                let cached = make_backend();
                cached
                    .adopt(&bound.runtime, &bound.request)
                    .await
                    .unwrap()
                    .unwrap();
                let mut samples = Vec::new();
                let mut reads = Vec::new();
                let mut assignments = Vec::new();
                for _ in 0..20 {
                    let new_backend = make_backend();
                    let backend = if restart { &new_backend } else { &cached };
                    effects.calls.lock().unwrap().clear();
                    let before = bound.fixture.state.lock().unwrap().assignments.len();
                    let start = Instant::now();
                    let observation = if persisted {
                        backend
                            .adopt_with_home(&bound.runtime, &bound.request, &home)
                            .await
                    } else {
                        backend.adopt(&bound.runtime, &bound.request).await
                    }
                    .unwrap()
                    .unwrap();
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    assert_eq!(observation.handle, bound.runtime);
                    assert!(observation.lifecycle.is_running());
                    reads.push(effects.calls.lock().unwrap().len());
                    assignments
                        .push(bound.fixture.state.lock().unwrap().assignments.len() - before);
                    assert_eq!(*assignments.last().unwrap(), usize::from(restart));
                }
                let mut sorted = samples.clone();
                sorted.sort_by(f64::total_cmp);
                eprintln!(
                    "{}",
                    json!({"fixture":"actual_adoption_delayed_reads", "home_mode":format!("{mode:?}"),
                    "restart":restart,"persisted":persisted,"n":20,"each_get_delay_ms":20,
                    "samples_ms":samples,"read_counts":reads,"bootstrap_counts":assignments,
                    "p50_ms":sorted[9],"p95_ms":sorted[18],"max_ms":sorted[19],
                    "production_or_full_create_measurement":false})
                );
            }
        }
    }
}
