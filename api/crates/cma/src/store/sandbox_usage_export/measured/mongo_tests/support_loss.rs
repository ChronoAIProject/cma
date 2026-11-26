use super::refresh::{assert_hold, head, retry};
use super::*;
use layer2_core::sandbox_usage_evidence::MeasuredQuality;

async fn save_binding(f: &Fixture, binding: &Binding) {
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .replace_one(doc! {"_id":&binding.id}, binding_document(binding))
        .upsert(true)
        .await
        .unwrap();
    let key = format!("{}{:016x}", keys::bridge(&binding.id), binding.valid_from);
    f.store
        .collection(INVENTORY)
        .replace_one(
            doc! {"_id":keys::id(&key)},
            document(
                &key,
                &BindingProof {
                    binding: binding.clone(),
                    resource: f.stream.resource.clone(),
                    verified_from: binding.valid_from,
                    verified_until: binding.valid_until.unwrap(),
                },
            )
            .unwrap(),
        )
        .upsert(true)
        .await
        .unwrap();
}

fn series(f: &Fixture, binding: &Binding, from: u64, to: u64, delta: f64) -> PhysicalSeries {
    let mut series = f.samples(delta).remove(0);
    series.name = binding.pod_name.clone().unwrap();
    series.samples[0].at = from as f64;
    series.samples[1].at = to as f64;
    series
}

pub(super) async fn publish(
    f: &Fixture,
    samples: Vec<PhysicalSeries>,
) -> (bool, super::super::freeze::MeasuredPublication) {
    let lease = f.lease().await;
    let publication = f.input(&lease).await.freeze(samples, true).unwrap();
    let published = f
        .store
        .publish_measured(&lease, &publication)
        .await
        .unwrap();
    f.store.release_measured(&lease).await.unwrap();
    (published, publication)
}

async fn stopped(f: &Fixture) {
    let row = f.rows(CAPTURES).await.remove(0);
    let mut capture: Captured = decode(&row).unwrap();
    capture.source.allocation.running = false;
    capture.source.next_allocation.running = false;
    capture.source.status = "stopped".into();
    capture.source.quantity = capture.source.allocation.usage(120).unwrap();
    f.store.collection(CAPTURES).update_one(doc! {"_id":row.get_str("_id").unwrap()},doc! {"$set":{
        "record":encode(&capture).unwrap(),"source_digest":hash(&serde_json::to_vec(&capture).unwrap())
    }}).await.unwrap();
}

fn another(mut binding: Binding, name: &str) -> Binding {
    binding.id = name.into();
    binding.pod_name = Some(format!("pod-{name}"));
    binding.pod_uid = Some(format!("uid-{name}"));
    binding.runtime_container_id = Some(format!("container-{name}"));
    binding
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn loss_of_one_observed_binding_and_equal_coverage_substitution_are_held() {
    let f = Fixture::new().await;
    let first = f.bind().await;
    let second = another(first.clone(), "second-fixture");
    save_binding(&f, &second).await;
    let samples = |binding, delta| series(&f, binding, f.minute, f.minute + 60, delta);
    let (published, original) =
        publish(&f, vec![samples(&first, 60.0), samples(&second, 60.0)]).await;
    assert!(published);
    let cpu = &original.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.quantity.as_deref(), Some("120000000000"));
    assert_eq!(cpu.covered_binding_ms, "120000");
    assert_eq!(cpu.quality, MeasuredQuality::Observed);
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .delete_one(doc! {"_id":&second.id})
        .await
        .unwrap();
    let (published, candidate) = publish(&f, vec![samples(&first, 60.0)]).await;
    let cpu = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.quantity.as_deref(), Some("60000000000"));
    assert_eq!(cpu.quality, MeasuredQuality::Observed);
    assert!(!published);
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;

    let substitute = another(second.clone(), "substitute-fixture");
    save_binding(&f, &substitute).await;
    retry(&f).await;
    let (published, candidate) =
        publish(&f, vec![samples(&first, 60.0), samples(&substitute, 30.0)]).await;
    let cpu = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.covered_binding_ms, "120000");
    assert_eq!(cpu.quantity.as_deref(), Some("90000000000"));
    assert_eq!(cpu.quality, MeasuredQuality::Observed);
    assert!(!published);
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;

    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .delete_one(doc! {"_id":&substitute.id})
        .await
        .unwrap();
    save_binding(&f, &second).await;
    retry(&f).await;
    assert!(
        publish(&f, vec![samples(&first, 60.0), samples(&second, 30.0)])
            .await
            .0
    );
    assert_eq!(head(&f).await.revision, before.revision + 1);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_residual_incarnation_cannot_promote_partial_evidence_to_observed() {
    let f = Fixture::new().await;
    stopped(&f).await;
    let mut first = f.bind().await;
    first.valid_until = Some(f.minute + 30);
    save_binding(&f, &first).await;
    let mut second = another(first.clone(), "residual-fixture");
    second.valid_from = f.minute + 30;
    second.valid_until = Some(f.minute + 120);
    save_binding(&f, &second).await;
    let later = series(&f, &second, f.minute + 30, f.minute + 60, 30.0);
    let (published, original) = publish(
        &f,
        vec![
            series(&f, &first, f.minute, f.minute + 29, 29.0),
            later.clone(),
        ],
    )
    .await;
    assert!(published);
    let fact = original.manifest.frozen.as_ref().unwrap().fact();
    assert_eq!(fact.compute_uptime_ms, 0);
    assert_eq!(fact.metrics[0].quantity.as_deref(), Some("59000000000"));
    assert_eq!(fact.metrics[0].covered_binding_ms, "59000");
    assert_eq!(fact.metrics[0].quality, MeasuredQuality::Partial);
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .delete_one(doc! {"_id":&first.id})
        .await
        .unwrap();
    let (published, candidate) = publish(&f, vec![later]).await;
    let cpu = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.quantity.as_deref(), Some("30000000000"));
    assert_eq!(cpu.quality, MeasuredQuality::Observed);
    assert!(!published);
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn equal_duration_on_another_interval_cannot_replace_retained_support() {
    let f = Fixture::new().await;
    stopped(&f).await;
    let mut binding = f.bind().await;
    binding.valid_until = Some(f.minute + 30);
    save_binding(&f, &binding).await;
    let mut samples = series(&f, &binding, f.minute, f.minute + 30, 0.0);
    samples.metric = SourceMetric::WorkingSet;
    let (published, original) = publish(&f, vec![samples]).await;
    assert!(published);
    let memory = &original.manifest.frozen.as_ref().unwrap().fact().metrics[1];
    assert_eq!(memory.covered_binding_ms, "30000");
    assert_eq!(memory.quantity.as_deref(), Some("300000"));
    assert_eq!(memory.quality, MeasuredQuality::Observed);
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    let old_bridge = format!("{}{:016x}", keys::bridge(&binding.id), f.minute);
    f.store
        .collection(INVENTORY)
        .delete_one(doc! {"_id":keys::id(&old_bridge)})
        .await
        .unwrap();
    binding.revision += 1;
    binding.valid_from = f.minute + 30;
    binding.valid_until = Some(f.minute + 60);
    save_binding(&f, &binding).await;
    let mut samples = series(&f, &binding, f.minute + 30, f.minute + 60, 0.0);
    samples.metric = SourceMetric::WorkingSet;
    samples.samples[0].value = 5.0;
    samples.samples[1].value = 5.0;
    let (published, candidate) = publish(&f, vec![samples]).await;
    let memory = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[1];
    assert_eq!(memory.covered_binding_ms, "30000");
    assert_eq!(memory.quantity.as_deref(), Some("150000"));
    assert_eq!(memory.quality, MeasuredQuality::Observed);
    assert!(!published);
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    f.db.drop().await.unwrap();
}
