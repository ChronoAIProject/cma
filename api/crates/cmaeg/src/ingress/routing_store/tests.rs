use super::{codec, test_server::Server, types::*, *};
use crate::ingress::launch::LaunchRequest;
use serde_json::json;
use std::sync::{atomic::Ordering, Arc};

pub(super) use crate::ingress::routing_plan::fixture;

fn custom(size: usize) -> (LaunchRequest, Vec<u8>) {
    let mut source = json!({"specversion":"1.0","id":"event-1","source":"https://producer.example/build","type":"build.done","data":""});
    let overhead = serde_json::to_vec(&source).unwrap().len();
    source["data"] = "x".repeat(size.saturating_sub(overhead)).into();
    let raw = serde_json::to_vec(&source).unwrap();
    let mut activity = fixture::custom(None);
    activity.kind = serde_json::from_value(json!({"type":"event","ce":source})).unwrap();
    (fixture::request(fixture::targets, activity), raw)
}
fn candidate(request: &LaunchRequest, raw: &[u8]) -> Candidate {
    Candidate::prepare(
        request,
        &fixture::routed(request),
        raw,
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap()
}

#[tokio::test]
async fn large_and_full_custom_payloads_survive_restart_without_losing_source_bytes() {
    for size in [70 * 1024, MAX_SOURCE_BYTES] {
        let server = Server::start().await;
        let (request, raw) = custom(size);
        assert_eq!(raw.len(), size);
        let upload = candidate(&request, &raw);
        assert!(upload.bytes.len() > 65_536);
        let plan = server.store().admit(&upload).await.unwrap();
        assert_eq!(plan.source_body, raw);
        assert_eq!(plan.activity, fixture::routed(&request).activity);
        let restored = server
            .store()
            .load(upload.identity())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored, plan);
        let rows = server.memory.rows.lock().unwrap();
        assert_eq!(rows.len(), upload.manifest.chunk_hashes.len() + 1);
        assert!(rows.values().all(|row| row.blob.len() <= 65_536));
        assert!(rows
            .values()
            .all(|row| row.expiry == fixture::NOW + RETENTION_SECONDS));
        assert!(!format!("{plan:?}").contains("producer.example"));
    }
}

#[tokio::test]
async fn lost_chunk_or_manifest_response_is_read_back_before_admission_succeeds() {
    for manifest in [false, true] {
        let server = Server::start().await;
        let (request, raw) = custom(70 * 1024);
        let upload = candidate(&request, &raw);
        let id = if manifest {
            upload.admission_id().into()
        } else {
            upload.manifest.chunk_id(1)
        };
        server.fault(id, true, false);
        let first = server.store().admit(&upload).await.unwrap();
        let writes = server.memory.writes.load(Ordering::SeqCst);
        assert_eq!(server.store().admit(&upload).await.unwrap(), first);
        assert_eq!(server.memory.writes.load(Ordering::SeqCst), writes);
    }
}

#[tokio::test]
async fn uncertain_writes_recover_as_partial_or_committed_without_false_acceptance() {
    for manifest in [false, true] {
        let server = Server::start().await;
        let (request, raw) = custom(70 * 1024);
        let upload = candidate(&request, &raw);
        let id = if manifest {
            upload.admission_id().into()
        } else {
            upload.manifest.chunk_id(1)
        };
        server.fault(id, true, true);
        assert!(server.store().admit(&upload).await.is_err());
        let state = server.store().inspect(&upload).await.unwrap();
        if manifest {
            assert!(matches!(state, AdmissionState::Committed(_)));
        } else {
            assert_eq!(
                state,
                AdmissionState::Partial {
                    present: 2,
                    total: upload.manifest.chunk_hashes.len()
                }
            );
            assert!(server
                .store()
                .load(upload.identity())
                .await
                .unwrap()
                .is_none());
        }
        assert_eq!(
            server.store().admit(&upload).await.unwrap().source_body,
            raw
        );
    }
}

#[tokio::test]
async fn failed_chunk_write_never_commits_root_and_original_body_can_retry() {
    let server = Server::start().await;
    let (request, raw) = custom(70 * 1024);
    let upload = candidate(&request, &raw);
    server.fault(upload.manifest.chunk_id(1), false, false);
    assert_eq!(
        server.store().admit(&upload).await.unwrap_err(),
        AdmissionError::Uncertain
    );
    assert!(!server
        .memory
        .rows
        .lock()
        .unwrap()
        .contains_key(upload.admission_id()));
    assert!(matches!(
        server.store().inspect(&upload).await.unwrap(),
        AdmissionState::Partial { present: 1, .. }
    ));
    assert_eq!(
        server.store().admit(&upload).await.unwrap().source_body,
        raw
    );
}

#[tokio::test]
async fn duplicate_concurrent_plans_commit_only_one_original_routing_snapshot() {
    let server = Server::start().await;
    let (mut request, raw) = custom(70 * 1024);
    let first = candidate(&request, &raw);
    Arc::get_mut(&mut request.channel)
        .unwrap()
        .definition
        .routing
        .as_mut()
        .unwrap()
        .default_targets = vec![fixture::id(fixture::REVIEWER)];
    let later = candidate(&request, &raw);
    assert_eq!(first.request_hash(), later.request_hash());
    assert_ne!(first.manifest.payload_hash, later.manifest.payload_hash);
    let one = server.store();
    let two = server.store();
    let (one, two) = tokio::join!(one.admit(&first), two.admit(&later));
    assert_eq!(one.unwrap(), two.unwrap());
    let frozen = server
        .store()
        .load(first.identity())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(server.store().admit(&later).await.unwrap(), frozen);
}

#[tokio::test]
async fn changed_body_with_same_event_conflicts_even_under_concurrent_admission() {
    let server = Server::start().await;
    let (request, raw) = custom(70 * 1024);
    let first = candidate(&request, &raw);
    let mut second_raw = raw.clone();
    let last_x = second_raw.iter().rposition(|byte| *byte == b'x').unwrap();
    second_raw[last_x] = b'y';
    let second = candidate(&request, &second_raw);
    assert_eq!(first.admission_id(), second.admission_id());
    assert_ne!(first.request_hash(), second.request_hash());
    let one = server.store();
    let two = server.store();
    let (one, two) = tokio::join!(one.admit(&first), two.admit(&second));
    assert!(matches!(
        (&one, &two),
        (Ok(_), Err(AdmissionError::Conflict)) | (Err(AdmissionError::Conflict), Ok(_))
    ));
    assert_eq!(
        server
            .store()
            .recover_page(&first.identity().owner, None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
}

#[tokio::test]
async fn committed_missing_corrupt_or_reordered_chunks_fail_closed_without_repair_writes() {
    for damage in ["missing", "ciphertext", "index", "order", "root_owner"] {
        let server = Server::start().await;
        let (request, raw) = custom(70 * 1024);
        let upload = candidate(&request, &raw);
        server.store().admit(&upload).await.unwrap();
        let chunk_id = upload.manifest.chunk_id(0);
        match damage {
            "missing" => {
                server.memory.rows.lock().unwrap().remove(&chunk_id);
            }
            "ciphertext" => {
                server
                    .memory
                    .rows
                    .lock()
                    .unwrap()
                    .get_mut(&chunk_id)
                    .unwrap()
                    .blob[20] ^= 1;
            }
            "index" => server.change(&chunk_id, |value| value["index"] = 1.into()),
            "order" => server.change(upload.admission_id(), |value| {
                value["chunk_hashes"].as_array_mut().unwrap().swap(0, 1)
            }),
            _ => server.change(upload.admission_id(), |value| {
                value["identity"]["owner"] = "other-owner".into()
            }),
        }
        let writes = server.memory.writes.load(Ordering::SeqCst);
        assert!(matches!(
            server.store().load(upload.identity()).await,
            Err(AdmissionError::MissingChunk | AdmissionError::Corrupt)
        ));
        assert!(server.store().admit(&upload).await.is_err());
        assert_eq!(server.memory.writes.load(Ordering::SeqCst), writes);
    }
}

#[tokio::test]
async fn encrypted_record_cannot_be_copied_across_chunk_or_owner_identity() {
    let server = Server::start().await;
    let (request, raw) = custom(70 * 1024);
    let upload = candidate(&request, &raw);
    server.store().admit(&upload).await.unwrap();
    let mut other = upload.identity().clone();
    other.owner = "another-owner".into();
    assert_ne!(other.admission_id(), upload.admission_id());
    assert!(server.store().load(&other).await.unwrap().is_none());
    let row = server
        .memory
        .rows
        .lock()
        .unwrap()
        .get(upload.admission_id())
        .unwrap()
        .clone();
    server
        .memory
        .rows
        .lock()
        .unwrap()
        .insert(other.admission_id(), row);
    assert_eq!(
        server.store().load(&other).await.unwrap_err(),
        AdmissionError::Corrupt
    );
    let row = server
        .memory
        .rows
        .lock()
        .unwrap()
        .get(&upload.manifest.chunk_id(0))
        .unwrap()
        .clone();
    server
        .memory
        .rows
        .lock()
        .unwrap()
        .insert(upload.manifest.chunk_id(1), row);
    assert_eq!(
        server.store().load(upload.identity()).await.unwrap_err(),
        AdmissionError::Corrupt
    );
}

#[tokio::test]
async fn retention_does_not_extend_event_authority_or_allow_expired_retry_admission() {
    let server = Server::start().await;
    let (request, raw) = custom(1024);
    let upload = candidate(&request, &raw);
    let plan = server.store().admit(&upload).await.unwrap();
    server
        .memory
        .clock
        .store(upload.event_expires_at, Ordering::SeqCst);
    assert_eq!(
        server
            .store()
            .load(upload.identity())
            .await
            .unwrap()
            .unwrap(),
        plan
    );
    let writes = server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        server.store().admit(&upload).await.unwrap_err(),
        AdmissionError::Expired
    );
    server
        .memory
        .clock
        .store(upload.manifest.retained_until, Ordering::SeqCst);
    assert!(server
        .store()
        .load(upload.identity())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        server.store().admit(&upload).await.unwrap_err(),
        AdmissionError::Expired
    );
    server.memory.rows.lock().unwrap().clear(); // Mongo TTL deletion does not revive the candidate.
    assert_eq!(
        server.store().admit(&upload).await.unwrap_err(),
        AdmissionError::Expired
    );
    assert_eq!(server.memory.writes.load(Ordering::SeqCst), writes);
    assert!(plan
        .targets
        .iter()
        .all(|target| target.launch.event.expires_at == upload.event_expires_at));
}

#[tokio::test]
async fn bounded_recovery_paginates_past_chunks_and_other_interaction_kinds() {
    let server = Server::start().await;
    let (request, raw) = custom(70 * 1024);
    let upload = candidate(&request, &raw);
    let expected = server.store().admit(&upload).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let blob =
        cmaeg_core::seal::seal_interaction(&fixture::keys(), &id, br#"{"kind":"unrelated"}"#)
            .unwrap();
    server.memory.rows.lock().unwrap().insert(
        id,
        super::test_server::Row {
            owner: upload.identity().owner.clone(),
            blob,
            expiry: upload.manifest.retained_until,
            version: 7,
        },
    );
    let mut cursor = None;
    let mut plans = Vec::new();
    loop {
        let page = server
            .store()
            .recover_page(&upload.identity().owner, cursor.as_deref(), 1)
            .await
            .unwrap();
        assert!(page.issues.is_empty());
        plans.extend(page.plans);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(plans, [expected]);
    assert!(server
        .store()
        .recover_page("unrelated-owner", None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    assert!(server
        .store()
        .recover_page(&upload.identity().owner, None, 33)
        .await
        .is_err());
}

#[test]
fn size_bounds_request_hashing_and_domain_separation_are_explicit() {
    let (request, raw) = custom(1024);
    let first = candidate(&request, &raw);
    let pretty =
        serde_json::to_vec_pretty(&serde_json::from_slice::<serde_json::Value>(&raw).unwrap())
            .unwrap();
    let second = candidate(&request, &pretty);
    assert_eq!(first.request_hash(), second.request_hash());
    assert_ne!(first.manifest.payload_hash, second.manifest.payload_hash); // exact source retained
    let too_large = vec![0; MAX_SOURCE_BYTES + 1];
    assert!(matches!(
        Candidate::prepare(
            &request,
            &fixture::routed(&request),
            &too_large,
            &fixture::keys(),
            fixture::NOW
        ),
        Err(AdmissionError::TooLarge)
    ));
    assert_ne!(first.admission_id(), first.manifest.chunk_id(0));
    let mut other = first.identity().clone();
    other.record_id = cmaeg_core::ids::RecordId::new("other-record").unwrap();
    assert_ne!(first.admission_id(), other.admission_id());
    other = first.identity().clone();
    other.event_id = fixture::event_id("other-event");
    assert_ne!(first.admission_id(), other.admission_id());
    assert!(codec::encode(&first.manifest).unwrap().len() < cmaeg_core::seal::MAX_BLOB_BYTES);
}
