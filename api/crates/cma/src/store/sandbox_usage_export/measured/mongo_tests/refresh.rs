use super::refresh_source::{Response, Source};
use super::*;

async fn stream(f: &Fixture) -> MeasuredStream {
    MeasuredStream::read(
        &f.store
            .collection(INVENTORY)
            .find_one(doc! {
                "_id":keys::id(&keys::stream("fixture", &f.stream.id))
            })
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

pub(super) async fn head(f: &Fixture) -> FactHead {
    decode(
        &f.store
            .collection(INVENTORY)
            .find_one(doc! {"_id":keys::id(&keys::head(&stream(f).await.id, f.minute))})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}

pub(super) async fn retry(f: &Fixture) {
    f.store
        .collection(INVENTORY)
        .update_one(
            doc! {"_id":keys::id(&keys::stream("fixture", &f.stream.id))},
            doc! {"$set":{"record.replay_turn":true,"record.replay_minute":f.minute as i64}},
        )
        .await
        .unwrap();
}

pub(super) async fn assert_hold(f: &Fixture, reason: &str, before: &FactHead, outbox: &[Document]) {
    assert_eq!(head(f).await, *before);
    assert_eq!(f.rows(OUTBOX).await, outbox);
    let row = f
        .store
        .collection(INVENTORY)
        .find_one(doc! {
            "_id":keys::id(&keys::hold(&stream(f).await.id, f.minute))
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("reason").unwrap(), reason);
    let proof = f
        .store
        .collection(EVIDENCE)
        .find_one(doc! {
            "_id":format!("measured:{}:{}", stream(f).await.id, row.get_str("evidence_id").unwrap())
        })
        .await
        .unwrap()
        .unwrap();
    let manifest: super::super::freeze::Manifest =
        serde_json::from_slice(proof.get_binary_generic("manifest").unwrap()).unwrap();
    assert_eq!(manifest.hold.as_deref(), Some(reason));
    assert_eq!(manifest.input.predecessor.as_ref(), Some(before));
    let lease = f.lease().await;
    assert_eq!(lease.stream.sequence, outbox.len() as u64);
    assert_eq!(lease.stream.replay_minute, f.minute);
    f.store.release_measured(&lease).await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worker_timeout_preserves_head_bytes_and_retry_then_accepts_downward_correction() {
    let f = Fixture::new().await;
    f.bind().await;
    let source = Source::new(&f).await;
    assert!(source.run(&f).await);
    let events = f.events().await;
    assert_eq!(
        events[1]["facts"][0]["quantities"]["sandbox_cpu_core_nanoseconds_v1"],
        "60000000000"
    );
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    let proof = f.rows(EVIDENCE).await;
    for response in [Response::TotalTimeout, Response::MemoryTimeout] {
        retry(&f).await;
        source.set(response);
        assert!(!source.run(&f).await);
        assert_eq!(source.calls(), ["cpu", "memory"]);
        assert_hold(&f, "refresh_source_unavailable", &before, &outbox).await;
        assert!(f.rows(EVIDENCE).await.contains(&proof[0]));
    }

    source.set(Response::Values(60.0));
    retry(&f).await;
    assert!(!source.run(&f).await);
    assert_eq!(head(&f).await, before);
    assert_eq!(f.rows(OUTBOX).await, outbox);
    assert!(!f
        .rows(INVENTORY)
        .await
        .iter()
        .any(|r| r.get_str("reason").is_ok()));
    for (delta, quantity) in [(30.0, "30000000000"), (0.0, "0")] {
        retry(&f).await;
        source.set(Response::Values(delta));
        assert!(source.run(&f).await);
        let events = f.events().await;
        let correction = &events.last().unwrap()["correction"];
        assert_eq!(correction["fact_id"], before.fact_id);
        assert_eq!(
            correction["replacement"]["quantities"]["sandbox_cpu_core_nanoseconds_v1"],
            quantity
        );
        assert_eq!(
            correction["replacement"]["coverage"]["cpu"]["quality"],
            "observed"
        );
    }
    assert_eq!(head(&f).await.revision, before.revision + 2);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worker_empty_gapped_and_erased_sources_cannot_withdraw_retained_support() {
    let f = Fixture::new().await;
    f.bind().await;
    let source = Source::new(&f).await;
    assert!(source.run(&f).await);
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    for response in [Response::Empty, Response::CpuGap] {
        retry(&f).await;
        source.set(response);
        assert!(!source.run(&f).await);
        assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    }
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .delete_many(doc! {})
        .await
        .unwrap();
    retry(&f).await;
    source.set(Response::Values(60.0));
    assert!(!source.run(&f).await);
    assert!(source.calls().is_empty());
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worker_explicit_newer_attribution_invalidation_can_replace_observed_usage() {
    let f = Fixture::new().await;
    let mut binding = f.bind().await;
    let source = Source::new(&f).await;
    assert!(source.run(&f).await);
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    binding.proof_status = ProofStatus::Unattributed;
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .replace_one(doc! {"_id":&binding.id}, binding_document(&binding))
        .await
        .unwrap();
    assert!(!source.run(&f).await);
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    binding.revision += 1;
    retry(&f).await;
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .replace_one(doc! {"_id":&binding.id}, binding_document(&binding))
        .await
        .unwrap();
    assert!(source.run(&f).await);
    let events = f.events().await;
    let replacement = &events[2]["correction"]["replacement"];
    assert_eq!(replacement["fact_id"], before.fact_id);
    assert!(replacement["quantities"]["sandbox_cpu_core_nanoseconds_v1"].is_null());
    assert_eq!(
        replacement["coverage"]["cpu"]["missing_reason"],
        "binding_missing"
    );
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn worker_first_incomplete_observation_is_null_not_zero() {
    let f = Fixture::new().await;
    f.bind().await;
    let source = Source::new(&f).await;
    source.set(Response::MemoryTimeout);
    assert!(source.run(&f).await);
    let events = f.events().await;
    let fact = &events[1]["facts"][0];
    assert!(fact["quantities"]["sandbox_cpu_core_nanoseconds_v1"].is_null());
    assert_eq!(fact["coverage"]["cpu"]["quality"], "missing");
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn submillisecond_support_is_not_erased_when_coverage_rounds_to_zero() {
    let f = Fixture::new().await;
    f.bind().await;
    let lease = f.lease().await;
    let mut samples = f.samples(1.0);
    samples[0].samples[1].at = f.minute as f64 + 0.000_1;
    let publication = f.input(&lease).await.freeze(samples, true).unwrap();
    let cpu = &publication.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.covered_binding_ms, "0");
    assert_eq!(cpu.quantity.as_deref(), Some("1000000000"));
    assert_eq!(
        cpu.quality,
        layer2_core::sandbox_usage_evidence::MeasuredQuality::Estimated
    );
    assert!(f
        .store
        .publish_measured(&lease, &publication)
        .await
        .unwrap());
    f.store.release_measured(&lease).await.unwrap();
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    let lease = f.lease().await;
    let publication = f.input(&lease).await.freeze(vec![], true).unwrap();
    assert!(!f
        .store
        .publish_measured(&lease, &publication)
        .await
        .unwrap());
    f.store.release_measured(&lease).await.unwrap();
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    f.db.drop().await.unwrap();
}
