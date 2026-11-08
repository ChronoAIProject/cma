use super::*;
use futures_util::{stream, StreamExt};

#[tokio::test]
async fn subscription_lifetime_covers_unpolled_drop_completion_and_cancellation() {
    isolated_test(async {
        let kind = Kind::Agui;
        let unpolled = observe(stream::pending::<()>(), kind);
        assert_eq!(snapshot(kind).0, 1);
        drop(unpolled);
        assert_eq!(snapshot(kind).0, 0);
        let mut finite = observe(stream::iter([1]), kind);
        assert_eq!(finite.next().await, Some(1));
        assert_eq!(snapshot(kind).0, 1);
        assert_eq!(finite.next().await, None);
        assert_eq!(snapshot(kind).0, 0);
        drop(finite);
        assert_eq!(snapshot(kind).0, 0);
        let mut pending = observe(stream::pending::<()>(), kind);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), pending.next())
                .await
                .is_err()
        );
        assert_eq!(snapshot(kind).0, 1);
        drop(pending);
        assert_eq!(snapshot(kind).0, 0);
    })
    .await;
}

#[tokio::test]
async fn failures_and_cancelled_refreshes_never_count_as_quiet_successes() {
    isolated_test(async {
        let kind = Kind::Responses;
        Poll::new(kind).complete(false);
        Poll::new(kind).complete(true);
        drop(Poll::new(kind));
        let refresh = async {
            let _poll = Poll::new(kind);
            std::future::pending::<()>().await;
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), refresh)
                .await
                .is_err()
        );
        assert_eq!(snapshot(kind), (0, [1, 1, 2]));
        assert_eq!(snapshot(Kind::ResponsesV2), (0, [0; 3]));
    })
    .await;
}

#[test]
fn labels_are_closed_and_counter_saturation_cannot_affect_streams() {
    assert_eq!(
        LABELS,
        [
            "agui",
            "agent_events_v2",
            "responses",
            "responses_v2",
            "launch_events"
        ]
    );
    assert_eq!(OUTCOMES, ["quiet", "output", "failed"]);
    let counter = AtomicU64::new(u64::MAX);
    increment(&counter);
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
}
