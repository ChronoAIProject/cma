use super::*;
use mongodb::bson::{doc, RawDocumentBuf};
use std::sync::{atomic::Ordering, Arc};

fn row(id: &str, n: i64) -> RawDocumentBuf {
    RawDocumentBuf::try_from(&doc! {"_id":id,"revision":1_i64,"sidecar":n}).unwrap()
}
fn candidate(id: &str) -> AgentConnectionRecoveryCandidate {
    AgentConnectionRecoveryCandidate {
        agent_id: layer2_core::AgentId(id.into()),
        sandbox_id: layer2_core::SandboxId("sbx_test".into()),
        conversation_id: "conversation".into(),
        runtime_generation: 1,
        revision: 1,
        work_step: layer2_core::sandbox::AgentWorkStep::None,
        recovery_eligible: true,
        managed_guidance_revision: 1,
        managed_context_eligible: true,
    }
}
fn observe(cache: &Observations, row: &RawDocument) {
    let id = row.get_str("_id").unwrap();
    assert_eq!(
        cache.resolve(row, || Ok(candidate(id))).unwrap(),
        candidate(id)
    );
}
fn bounds(cache: &Observations) -> (usize, usize) {
    let state = cache.state.lock().unwrap();
    assert_eq!(
        state.bytes,
        METADATA_BYTES + state.entries.values().map(|e| e.charge).sum::<usize>()
    );
    assert!(state.bytes <= MAX_BYTES);
    assert!(state.entries.len() <= MAX_ENTRIES);
    (state.entries.len(), state.bytes)
}

#[tokio::test(start_paused = true)]
async fn exact_bytes_and_fixed_age_revalidate_changed_sidecars_and_expiry() {
    let cache = Observations::default();
    let original = row("agt_a", 1);
    observe(&cache, &original);
    tokio::time::advance(Duration::from_secs(20)).await;
    observe(&cache, &original);
    assert_eq!(cache.hits.load(Ordering::Relaxed), 1);
    tokio::time::advance(Duration::from_secs(10)).await;
    observe(&cache, &original);
    assert_eq!(
        cache.validations.load(Ordering::Relaxed),
        2,
        "hits never extend age"
    );
    observe(&cache, &row("agt_a", 2));
    assert_eq!(
        cache.validations.load(Ordering::Relaxed),
        3,
        "same revision is insufficient"
    );
    observe(&cache, &original);
    assert_eq!(cache.validations.load(Ordering::Relaxed), 4);
    assert_eq!(bounds(&cache).0, 1);
}

#[test]
fn validation_errors_are_never_reused_and_failed_or_cancelled_pages_clear_state() {
    let cache = Observations::default();
    observe(&cache, &row("agt_a", 1));
    for _ in 0..2 {
        assert!(cache
            .resolve(&row("agt_a", 2), || Err(SandboxStoreError::Conflict))
            .is_err());
    }
    assert_eq!(bounds(&cache), (0, METADATA_BYTES));
    assert_eq!(cache.validations.load(Ordering::Relaxed), 3);
    observe(&cache, &row("agt_a", 1));
    let mut successful = cache.pass();
    successful.complete = true;
    drop(successful);
    assert_eq!(bounds(&cache).0, 1);
    drop(cache.pass());
    assert_eq!(bounds(&cache), (0, METADATA_BYTES));
    observe(&cache, &row("agt_a", 1));
    assert_eq!(cache.validations.load(Ordering::Relaxed), 5);
}

#[tokio::test(start_paused = true)]
async fn entry_limit_fails_open_and_expired_entries_release_exact_charges() {
    let cache = Observations::default();
    for n in 0..MAX_ENTRIES + 1 {
        observe(&cache, &row(&format!("agt_{n}"), 1));
    }
    assert_eq!(bounds(&cache).0, MAX_ENTRIES);
    let unretained = row(&format!("agt_{MAX_ENTRIES}"), 1);
    observe(&cache, &unretained);
    assert_eq!(cache.hits.load(Ordering::Relaxed), 0);
    tokio::time::advance(MAX_AGE).await;
    observe(&cache, &unretained);
    assert_eq!(bounds(&cache).0, 1);
    observe(&cache, &unretained);
    assert_eq!(cache.hits.load(Ordering::Relaxed), 1);
}

#[test]
fn byte_budget_and_oversized_replacements_fail_open_without_retained_payload() {
    let cache = Observations::default();
    let payload = "x".repeat(MAX_ROW_BYTES - 128);
    for n in 0..100 {
        let row =
            RawDocumentBuf::try_from(&doc! {"_id":format!("agt_{n}"),"payload":&payload}).unwrap();
        observe(&cache, &row);
    }
    let (entries, bytes) = bounds(&cache);
    assert!(entries > 0 && entries < 100);
    assert!(bytes > MAX_BYTES - MAX_ROW_BYTES);
    let too_large =
        RawDocumentBuf::try_from(&doc! {"_id":"agt_0","payload":"x".repeat(MAX_ROW_BYTES)})
            .unwrap();
    observe(&cache, &too_large);
    observe(&cache, &too_large);
    assert_eq!(bounds(&cache).0, entries - 1);
    assert_eq!(cache.hits.load(Ordering::Relaxed), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sessions_share_bounded_candidates_without_sharing_authority() {
    let cache = Arc::new(Observations::default());
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..200 {
        let cache = cache.clone();
        tasks.spawn(async move {
            let row = row(&format!("agt_{n}"), 1);
            observe(&cache, &row);
            observe(&cache, &row);
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(bounds(&cache).0, 200);
    assert_eq!(cache.hits.load(Ordering::Relaxed), 200);
    assert_eq!(cache.validations.load(Ordering::Relaxed), 200);
    let restart = Observations::default();
    observe(&restart, &row("agt_0", 1));
    assert_eq!(restart.hits.load(Ordering::Relaxed), 0);
}
