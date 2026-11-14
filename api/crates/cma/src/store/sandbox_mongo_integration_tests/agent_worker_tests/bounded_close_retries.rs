use super::*;
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD, QUARANTINE_FIELD};
use crate::store::agent_log_telemetry::{self as scans, ScanCaller};
use crate::store::mongo_read_telemetry as reads;

mod exclusions;
mod fixture;
mod recovery;
mod refusals;
mod repeated;
use fixture::{claim, retry, Fixture};
const CALLER: ScanCaller = ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess);

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_matches_full_publication_and_bounds_checkouts_and_scans() {
    for history in [1_000, 9_990] {
        let mut f = Fixture::new(history).await;
        let before_row = f.row().await;
        let mutation = retry(&f.agent);
        f.store = f.store.with_bounded_close_retries(false);
        let start = f.checkouts.load(Ordering::SeqCst);
        let baseline = scans::isolated_test(reads::isolated_test(async {
            f.store.commit_agent_work(mutation.clone()).await.unwrap();
            assert_eq!(scans::snapshot(CALLER, true)[0], 1);
            reads::snapshot(AGENT_LOG)
        }))
        .await;
        let original_checkouts = f.checkouts.load(Ordering::SeqCst) - start;
        let expected = f.row().await;
        f.reset(before_row).await;
        f.store = f.store.with_bounded_close_retries(true);
        let start = f.checkouts.load(Ordering::SeqCst);
        let bounded = scans::isolated_test(reads::isolated_test(async {
            f.store.commit_agent_work(mutation).await.unwrap();
            assert_eq!(scans::snapshot(CALLER, true)[0], 0);
            reads::snapshot(AGENT_LOG)
        }))
        .await;
        let checkouts = f.checkouts.load(Ordering::SeqCst) - start;
        assert_eq!(
            f.row().await,
            expected,
            "identical durable publication and integrity"
        );
        assert_eq!(bounded[2], 1, "only the exact proof tail is consumed");
        assert!(checkouts < original_checkouts);
        assert_eq!(baseline[2], history + 1);
        eprintln!("close_retry history={history} original_checkouts={original_checkouts} bounded_checkouts={checkouts} original_log_documents={} bounded_log_documents={} original_bytes={} bounded_bytes={}", baseline[2], bounded[2], baseline[3], bounded[3]);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn close_retry_untrusted_or_disabled_fallbacks_keep_complete_validation() {
    for mode in [
        "off",
        "cohort_off",
        "missing",
        "stale",
        "unsupported",
        "semantic_untrusted",
    ] {
        let mut f = Fixture::new(500).await;
        match mode {
            "off" => f.store = f.store.with_bounded_close_retries(false),
            "cohort_off" => f.store.bounded_admission_agents.clear(),
            "missing" => f.change(doc! { "$unset": { INTEGRITY_FIELD: "" }}).await,
            "stale" => f.change(doc! { "$inc": { "agent_log_integrity_v1.bound_agent_revision": -1_i64 }}).await,
            "unsupported" => f.change(doc! { "$set": { "agent_log_integrity_v1.format": 2_i64 }}).await,
            "semantic_untrusted" => f.change(doc! { "$set": { "agent_log_integrity_v1.semantic_checkpoint_v1.valid": false }}).await,
            _ => unreachable!(),
        }
        let start = f.checkouts.load(Ordering::SeqCst);
        let consumed = scans::isolated_test(reads::isolated_test(async {
            f.store.commit_agent_work(retry(&f.agent)).await.unwrap();
            assert_eq!(scans::snapshot(CALLER, true)[0], 1, "{mode}");
            reads::snapshot(AGENT_LOG)
        }))
        .await;
        let checkouts = f.checkouts.load(Ordering::SeqCst) - start;
        eprintln!(
            "close_retry fallback={mode} checkouts={checkouts} log_documents={} bytes={}",
            consumed[2], consumed[3]
        );
        assert_eq!(
            f.store.get_agent(&f.agent.id).await.unwrap(),
            Some(retry(&f.agent).agent)
        );
        f.cleanup().await;
    }
}
