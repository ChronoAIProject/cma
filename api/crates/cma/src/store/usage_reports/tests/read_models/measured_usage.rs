use super::*;
use crate::store::{
    sandbox_maintenance::mongo_integration_tests::{insert_agent, ready_agent},
    sandbox_mongo::MongoSandboxStore,
};
use axum::{extract::RawQuery, routing::get, Router};
use layer2_core::sandbox::{
    store::{SandboxEventStore, SandboxStore},
    RuntimeBinding, Sandbox, SandboxEventKind, SandboxHome, SandboxHomeRecovery, SandboxStatus,
    SandboxWorkStep,
};
use layer3_adapter::{
    sandbox_metrics::{MetricsClient, SourceMetric},
    HomeVolumePolicy, HomeVolumeReceipt,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU8, Ordering};

pub(in crate::store::usage_reports::tests) const PRIMARY: &str =
    "sbx_00000000000000000000000000000001";
pub(in crate::store::usage_reports::tests) const SECONDARY: &str =
    "sbx_00000000000000000000000000000002";
pub(in crate::store::usage_reports::tests) const GIB: u64 = 1_073_741_824;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ambiguous_metric_name_preserves_prior_rollups_and_replay_advances() {
    let store = fixture().await;
    let from = (now() / 3600 - 2) * 3600;
    let range = Range {
        from,
        to: from + 3600,
    };
    let ids: Vec<_> = (1..=101).map(|id| format!("sbx_{id:032x}")).collect();
    let observer = "reuse-fixture-observer";
    assert!(store.lease("observer", observer, 70).await.unwrap());
    for id in &ids {
        let mut compute = binding(id, BindingKind::Compute, from);
        compute.pod_name = Some(format!("{id}-pod"));
        compute.pod_uid = Some(format!("{id}-pod-uid"));
        compute.native_sandbox_uid = format!("{id}-native");
        compute.claim_uid = format!("{id}-claim");
        store
            .reconcile_bindings(observer, id, vec![compute.clone()], from)
            .await
            .unwrap();
        store
            .reconcile_bindings(observer, id, vec![compute], from + 90)
            .await
            .unwrap();
    }
    store.release("observer", observer).await.unwrap();

    let mode = Arc::new(AtomicU8::new(0));
    let source_mode = mode.clone();
    let source_ids = ids.clone();
    let router = Router::new().route(
        "/select/0/prometheus/api/v1/export",
        get(move |RawQuery(query): RawQuery| {
            let ids = source_ids.clone();
            let mode = source_mode.load(Ordering::Relaxed);
            async move {
                let query = query.unwrap();
                let selection = url::form_urlencoded::parse(query.as_bytes())
                    .find(|(key, _)| key == "match[]")
                    .unwrap()
                    .1
                    .into_owned();
                let metric = SourceMetric::ALL
                    .into_iter()
                    .find(|metric| selection.starts_with(metric.name()))
                    .unwrap();
                let rate = match mode {
                    0 => 1,
                    1 => 2,
                    _ => 3,
                };
                let mut rows = Vec::new();
                for id in ids
                    .iter()
                    .filter(|id| selection.contains(&format!("{id}-pod")))
                {
                    let mut row = export_line(from, id, metric, &[(0, 0), (60, 60 * rate)]);
                    row["metric"]["node"] = json!("current-node");
                    rows.push(row.to_string());
                    if mode == 1 && id == &ids[0] && metric == SourceMetric::CpuCounter {
                        let mut older = export_line(from, id, metric, &[(90, 150)]);
                        older["metric"]["node"] = json!("other-node");
                        rows.push(older.to_string());
                    }
                }
                rows.join("\n")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/select/0/prometheus/",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = MetricsClient::internal(&endpoint).unwrap();
    crate::sandbox_metrics::collect_fixture(store.clone(), client.clone(), &ids[..1], range)
        .await
        .unwrap();

    mode.store(1, Ordering::Relaxed);
    crate::sandbox_metrics::replay_fixture(store.clone(), client.clone(), range)
        .await
        .unwrap();
    let progress = store.progress("replay").await.unwrap();
    assert_eq!(progress.from, range.to);
    assert!(progress.after.is_none());
    let wanted = [ids[0].clone(), ids[1].clone(), ids[100].clone()];
    let read = || async {
        let mut snapshot = store.snapshot().await.unwrap();
        let rollups = store
            .metric_rollups(&mut snapshot, &wanted, range)
            .await
            .unwrap();
        rollups
            .into_iter()
            .map(|rollup| {
                let cpu = rollup
                    .minutes
                    .iter()
                    .map(|minute| minute.metrics[0].integral)
                    .sum::<f64>();
                (rollup.sandbox_id, cpu)
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let mixed = read().await;
    assert_eq!(mixed[&ids[0]], 60.);
    assert_eq!(mixed[&ids[1]], 120.);
    assert_eq!(mixed[&ids[100]], 120.);

    mode.store(2, Ordering::Relaxed);
    crate::sandbox_metrics::replay_fixture(store.clone(), client, range)
        .await
        .unwrap();
    let repaired = read().await;
    assert_eq!(repaired.len(), 3);
    assert!(repaired.values().all(|value| *value == 180.));
    server.abort();
    store.db.drop().await.unwrap();
}

pub(in crate::store::usage_reports::tests) async fn collected() -> (Arc<ReportingStore>, Range) {
    let store = fixture().await;
    let from = (now() - 3600) / 3600 * 3600;
    let range = Range {
        from,
        to: from + 300,
    };
    let meter = MongoSandboxStore::open(&store.client, store.db.name())
        .await
        .unwrap();
    let mut primary = sandbox(&meter, PRIMARY, "owner-1", from).await;
    let secondary = sandbox(&meter, SECONDARY, "owner-2", from).await;
    for id in [
        "agt_00000000000000000000000000000001",
        "agt_00000000000000000000000000000002",
    ] {
        let agent = ready_agent(&primary, id);
        insert_agent(&meter, &agent, &at(from)).await;
    }
    let restart = from + 120;
    primary.runtime = Some(runtime(PRIMARY, 2, restart));
    save(&meter, &mut primary, restart).await;
    let pause = from + 180;
    let pause_at = at(pause);
    let home = primary.lifecycle.home.as_ref().unwrap();
    primary.lifecycle.home_recovery = Some(Box::new(SandboxHomeRecovery {
        home_identity: home.attachment().unwrap().identity().unwrap(),
        source_generation: primary.lifecycle.runtime_generation_high_water,
        resume_pause: None,
    }));
    primary.runtime = None;
    primary.status = SandboxStatus::Paused;
    primary.credential.delivered = false;
    primary.credential.current = None;
    primary.work.schedule(SandboxWorkStep::None, pause_at);
    save(&meter, &mut primary, pause).await;

    let observer = "measured-binding-fixture";
    assert!(store.lease("observer", observer, 70).await.unwrap());
    // The previous identity closes at its last observation, leaving 30 seconds
    // between the last proof for generation 1 and the first for generation 2.
    for (offset, generation) in [
        (0, Some(1)),
        (90, Some(1)),
        (120, Some(2)),
        (180, Some(2)),
        (180, None),
        (240, None),
        (300, None),
    ] {
        observe(&store, observer, &primary, from + offset, generation).await;
    }
    for offset in [0, 60, 120, 180, 240, 300] {
        observe(&store, observer, &secondary, from + offset, Some(1)).await;
    }
    store.release("observer", observer).await.unwrap();

    let router = Router::new().route(
        "/select/0/prometheus/api/v1/export",
        get(move |RawQuery(query): RawQuery| async move {
            let query = query.unwrap();
            let selection = url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "match[]")
                .unwrap()
                .1
                .into_owned();
            let metric = SourceMetric::ALL
                .into_iter()
                .find(|metric| selection.starts_with(metric.name()))
                .unwrap();
            export(from, metric)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/select/0/prometheus/",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = crate::sandbox_metrics::collect_fixture(
        store.clone(),
        MetricsClient::internal(&endpoint).unwrap(),
        &[PRIMARY.into(), SECONDARY.into()],
        range,
    )
    .await;
    server.abort();
    result.unwrap();
    (store, range)
}

fn at(seconds: u64) -> String {
    layer2_core::rfc3339_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
}

async fn sandbox(meter: &MongoSandboxStore, id: &str, owner: &str, from: u64) -> Sandbox {
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(id);
    sandbox.name = id.into();
    sandbox.owner = SubjectId(owner.into());
    sandbox.created_at = at(from);
    sandbox.updated_at = at(from);
    sandbox.last_activity_at = at(from);
    sandbox.runtime = Some(runtime(id, 1, from));
    sandbox.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 10,
            template_name: "fixture-seed".into(),
            template_generation: "fixture-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: format!("{id}-home"),
        storage_generation: "fixture-home-generation".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    meter.create_sandbox(sandbox.clone()).await.unwrap();
    meter
        .append_sandbox_event(&sandbox.id, at(from), SandboxEventKind::Created)
        .await
        .unwrap();
    sandbox
}

fn runtime(id: &str, generation: u64, from: u64) -> RuntimeBinding {
    RuntimeBinding {
        runtime_id: format!("{id}-runtime-{generation}"),
        runtime_version: "fixture-image".into(),
        bound_at: at(from),
        boot_id: None,
        provision_generation: Some(generation),
    }
}

async fn save(meter: &MongoSandboxStore, sandbox: &mut Sandbox, time: u64) {
    let revision = sandbox.revision;
    sandbox.revision += 1;
    sandbox.updated_at = at(time);
    if let Some(runtime) = &sandbox.runtime {
        sandbox.lifecycle.runtime_generation_high_water = runtime.provision_generation.unwrap();
    }
    meter
        .save_sandbox(revision, sandbox.clone())
        .await
        .unwrap_or_else(|error| panic!("save {:?} Sandbox at {time}: {error}", sandbox.status));
}

async fn observe(
    store: &ReportingStore,
    observer: &str,
    sandbox: &Sandbox,
    at: u64,
    generation: Option<u64>,
) {
    let id = sandbox.id.as_str();
    let mut home = binding(id, BindingKind::Home, at);
    home.owner_subject = sandbox.owner.to_string();
    home.native_sandbox_uid = format!("{id}-native");
    home.claim_uid = format!("{id}-claim");
    home.pvc_name = format!("{id}-home");
    home.pvc_uid = format!("{id}-pvc");
    home.pod_name = None;
    home.pod_uid = None;
    home.runtime_container_id = None;
    home.runtime_generation = None;
    home.restart_count = None;
    home.cpu_limit_cores = None;
    home.memory_limit_bytes = None;
    let mut observed = vec![home.clone()];
    if let Some(generation) = generation {
        let mut compute = home;
        compute.kind = BindingKind::Compute;
        compute.pod_name = Some(format!("{id}-pod"));
        compute.pod_uid = Some(format!("{id}-pod-{generation}"));
        compute.runtime_container_id = Some(format!("containerd://{id}-{generation}"));
        compute.runtime_generation = Some(generation);
        compute.restart_count = Some(i32::try_from(generation - 1).unwrap());
        compute.cpu_limit_cores = Some(1.);
        compute.memory_limit_bytes = Some((2 * GIB) as f64);
        observed.push(compute);
    }
    store
        .reconcile_bindings(observer, id, observed, at)
        .await
        .unwrap();
}

fn export(from: u64, metric: SourceMetric) -> String {
    use SourceMetric::*;
    let primary: &[(u64, u64)] = match metric {
        CpuCounter => &[(0, 100), (30, 130), (60, 175), (120, 1000), (150, 1015)],
        WorkingSet => &[(0, GIB), (60, 2 * GIB), (120, 3 * GIB), (150, 2 * GIB)],
        PvcUsed => &[(0, 2 * GIB), (60, 3 * GIB), (180, 4 * GIB), (240, 5 * GIB)],
        PvcCapacity => &[
            (0, 10 * GIB),
            (60, 10 * GIB),
            (120, 10 * GIB),
            (180, 10 * GIB),
            (240, 10 * GIB),
        ],
    };
    let secondary: &[(u64, u64)] = match metric {
        CpuCounter => &[(0, 0), (60, 120), (120, 240), (180, 360), (240, 480)],
        WorkingSet => &[(0, GIB), (60, GIB), (120, GIB), (180, GIB), (240, GIB)],
        PvcUsed => &[
            (0, 8 * GIB),
            (60, 8 * GIB),
            (120, 8 * GIB),
            (180, 8 * GIB),
            (240, 8 * GIB),
        ],
        PvcCapacity => primary,
    };
    [(PRIMARY, primary), (SECONDARY, secondary)]
        .map(|(id, samples)| export_line(from, id, metric, samples).to_string())
        .join("\n")
}

fn export_line(from: u64, id: &str, metric: SourceMetric, samples: &[(u64, u64)]) -> Value {
    let mut labels = json!({
        "__name__": metric.name(),
        "cluster": "cluster",
        "namespace": "namespace",
        "job": "kubelet",
        "metrics_path": metric.metrics_path(),
    });
    let name = if metric.is_pvc() {
        format!("{id}-home")
    } else {
        labels["container"] = json!("runtime");
        format!("{id}-pod")
    };
    labels[metric.identity_label()] = json!(name);
    json!({
        "metric": labels,
        "timestamps": samples.iter().map(|(at, _)| (from + at) * 1000).collect::<Vec<_>>(),
        "values": samples.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
    })
}
