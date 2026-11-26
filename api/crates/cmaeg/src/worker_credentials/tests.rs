use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::seal::seal_blob;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Default)]
pub(crate) struct Memory {
    pub known: Mutex<BTreeSet<String>>,
    pub cutoff: Mutex<Option<Cutover>>,
    pub fail: AtomicBool,
    pub calls: AtomicUsize,
    pub hold: AtomicBool,
    pub started: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}
impl WorkerHistory for Memory {
    fn record<'a>(&'a self, hash: &'a str) -> BoxFuture<'a, Result<(), RecordError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            if self.hold.load(Ordering::SeqCst) {
                self.release.notified().await;
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(RecordError::Unavailable("fixture unavailable".into()));
            }
            self.known
                .lock()
                .unwrap()
                .insert(canonical_digest(hash)?.into());
            Ok(())
        })
    }
    fn was_worker<'a>(&'a self, hash: &'a str) -> BoxFuture<'a, Result<bool, RecordError>> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                return Err(RecordError::Unavailable("fixture unavailable".into()));
            }
            Ok(self.known.lock().unwrap().contains(canonical_digest(hash)?))
        })
    }
    fn cutover(&self) -> BoxFuture<'_, Result<Option<Cutover>, RecordError>> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                Err(RecordError::Unavailable("fixture unavailable".into()))
            } else {
                Ok(self.cutoff.lock().unwrap().clone())
            }
        })
    }
}
pub(crate) fn keys() -> Arc<MasterKeyRing> {
    Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([31; 32]))).unwrap())
}
pub(crate) fn cutoff() -> Cutover {
    Cutover {
        id: "owner-lane".into(),
        attestation_id: uuid::Uuid::new_v4().to_string(),
        not_before: 10,
        revision: 1,
        writer_release: "a".repeat(40),
        all_writers_tracked: true,
        retained_records_backfilled: true,
    }
}

#[test]
fn disabled_defaults_and_explicit_tracking_cutover_are_required() {
    let parsed = config::Settings::parse(|_| Ok(None)).unwrap();
    assert!(!parsed.tracking && !parsed.owner_keys);
    let values = [
        (OWNER_KEYS, "true"),
        (TRACKING, "true"),
        (SERVICE_ID, "service"),
        (CUTOVER_ID, "invalid"),
    ];
    assert!(config::Settings::parse(|name| Ok(values
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())))
    .is_err());
    for value in ["TRUE", "1", ""] {
        assert!(
            config::Settings::parse(|name| Ok((name == TRACKING).then(|| value.into()))).is_err()
        );
    }
}

#[test]
fn cutoff_rejects_old_parent_new_child_and_incomplete_or_stale_attestation() {
    let valid = cutoff();
    let id = valid.attestation_id.clone();
    assert!(valid.admits(&id, 11));
    for issued in [0, 9, 10] {
        assert!(!valid.admits(&id, issued));
    }
    assert!(!valid.admits("older-attestation", 11));
    let mut missing = valid.clone();
    missing.all_writers_tracked = false;
    assert!(!missing.admits(&id, 11));
    missing = valid.clone();
    missing.retained_records_backfilled = false;
    assert!(!missing.admits(&id, 11));
    missing = valid;
    missing.not_before = -1;
    assert!(!missing.admits(&id, 11));
}

#[test]
fn digest_formats_are_normalized_deliberately_and_malformed_values_never_unknown() {
    let digest = "a".repeat(64);
    assert_eq!(canonical_digest(&digest).unwrap(), digest);
    assert_eq!(
        canonical_digest(&format!("sha256:{digest}")).unwrap(),
        digest
    );
    for bad in [
        "A".repeat(64),
        "g".repeat(64),
        "a".repeat(63),
        format!("sha256:sha256:{digest}"),
    ] {
        assert!(canonical_digest(&bad).is_err());
    }
}

#[tokio::test]
async fn sealed_default_additional_and_pending_credentials_are_registered_and_never_removed() {
    use crate::channels::routing_fixture as fixture;
    let memory = Arc::new(Memory::default());
    let keys = keys();
    let tracking = WorkerCredentials::new(memory.clone(), keys.clone());
    let mut value = fixture::routed();
    value["routing"]["consumers"][1]["pending"] = true.into();
    let definition = fixture::definition(value);
    let blob = seal_blob(
        &keys,
        &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
    )
    .unwrap();
    tracking
        .record_blob(definition.record_id.as_str(), 1, &blob)
        .await
        .unwrap();
    assert_eq!(memory.known.lock().unwrap().len(), 3);
    let retired = crate::channels::definition::hash_secret(&fixture::agent_key(fixture::NYX));
    assert!(memory.was_worker(&retired).await.unwrap());
    let mut changed = definition.to_blob_json();
    changed["routing"]["consumers"] = serde_json::json!([]);
    changed["secrets"]["consumer_keys"] = serde_json::json!([]);
    let changed = fixture::definition(changed);
    let blob = seal_blob(&keys, &serde_json::to_vec(&changed.to_blob_json()).unwrap()).unwrap();
    tracking
        .record_blob(changed.record_id.as_str(), 2, &blob)
        .await
        .unwrap();
    assert!(memory.was_worker(&retired).await.unwrap());
    assert_eq!(memory.known.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn failed_or_uncertain_registration_cannot_populate_positive_cache() {
    let memory = Arc::new(Memory::default());
    let tracking = WorkerCredentials::new(memory.clone(), keys());
    memory.fail.store(true, Ordering::SeqCst);
    assert!(tracking.record_key("worker-fixture").await.is_err());
    assert!(tracking.durable_known.lock().unwrap().is_empty());
    memory.fail.store(false, Ordering::SeqCst);
    tracking.record_key("worker-fixture").await.unwrap();
    tracking.record_key("worker-fixture").await.unwrap();
    assert_eq!(memory.calls.load(Ordering::SeqCst), 2);
    assert_eq!(tracking.durable_known.lock().unwrap().len(), 1);
}
