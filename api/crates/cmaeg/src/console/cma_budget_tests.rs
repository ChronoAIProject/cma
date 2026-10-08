use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use super::*;
use crate::console::test_support::FakeHeads;

#[tokio::test]
async fn pages_are_listed_one_at_a_time_with_a_paced_pause() {
    let heads = FakeHeads::seeded(350);
    let budget = CmaBudget::default();
    let mut pages = Vec::new();
    let listed = page_all(&heads, &budget, Pacing::default(), None, |page| {
        pages.push(page.len());
    })
    .await
    .unwrap();
    assert_eq!(listed, 350);
    assert_eq!(pages, [100, 100, 100, 50]);
    assert_eq!(heads.max_inflight.load(Ordering::SeqCst), 1);
    let calls = heads.calls.lock().unwrap();
    for pair in calls.windows(2) {
        let gap = pair[1].1.duration_since(pair[0].2);
        assert!(gap >= Duration::from_millis(50), "{gap:?}");
        assert!(gap < Duration::from_millis(400), "{gap:?}");
    }
}

#[tokio::test]
async fn an_owner_listing_asks_cma_for_that_owner_only() {
    let heads = FakeHeads::seeded(10);
    let budget = CmaBudget::default();
    let listed = page_all(&heads, &budget, Pacing::default(), Some("bob"), |_| {})
        .await
        .unwrap();
    assert_eq!(listed, 5);
    let calls = heads.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.as_deref(), Some("bob"));
}

#[tokio::test]
async fn a_failed_page_ends_the_listing_with_its_error() {
    let heads = FakeHeads::seeded(10);
    *heads.fail.lock().unwrap() = true;
    let budget = CmaBudget::default();
    let error = page_all(&heads, &budget, Pacing::default(), None, |_| {
        panic!("a failed page is never handed on")
    })
    .await
    .unwrap_err();
    assert!(matches!(error, RecordError::Unavailable(_)));
}

#[tokio::test]
async fn at_most_four_console_reads_reach_cma_at_once() {
    let heads = Arc::new(FakeHeads {
        delay: Duration::from_millis(30),
        ..FakeHeads::seeded(1)
    });
    let budget = Arc::new(CmaBudget::default());
    let tasks: Vec<_> = (0..12)
        .map(|_| {
            let heads = heads.clone();
            let budget = budget.clone();
            tokio::spawn(async move {
                page_all(heads.as_ref(), &budget, Pacing::default(), None, |_| {})
                    .await
                    .unwrap()
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(heads.max_inflight.load(Ordering::SeqCst), PERMITS);
}

#[test]
fn pauses_stay_within_half_the_base_either_side() {
    let pacing = Pacing::default();
    for _ in 0..200 {
        let pause = pacing.pause();
        assert!(pause >= Duration::from_millis(50) && pause <= Duration::from_millis(150));
    }
}
