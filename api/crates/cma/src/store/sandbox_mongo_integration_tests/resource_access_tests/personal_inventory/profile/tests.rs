use super::*;
use std::{
    future::{poll_fn, Future},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

fn resolution(seconds: f64) {
    metrics::histogram!("cma_auth_resolution_seconds", "path" => "ambient", "outcome" => "resolved")
        .record(seconds);
}

#[tokio::test]
async fn per_poll_recorders_isolate_interleaved_requests_and_restore_the_outer_recorder() {
    let outer = identity::Recorder::default();
    let _recording = metrics::set_default_local_recorder(&outer);
    let first = Profile::default();
    let second = Profile::default();
    let (send_first, receive_first) = tokio::sync::oneshot::channel::<()>();
    let (send_second, receive_second) = tokio::sync::oneshot::channel::<()>();
    let mut a = Box::pin(first.measure(async {
        metrics::counter!("cma_identity_provider_calls_total").increment(2);
        receive_first.await.unwrap();
        resolution(0.003);
        17
    }));
    let mut b = Box::pin(second.measure(async {
        metrics::counter!("cma_identity_provider_calls_total").increment(6);
        receive_second.await.unwrap();
        resolution(0.007);
        23
    }));
    assert!(futures_util::poll!(a.as_mut()).is_pending());
    assert!(futures_util::poll!(b.as_mut()).is_pending());
    resolution(0.011);
    send_second.send(()).unwrap();
    assert_eq!(b.await, 23);
    send_first.send(()).unwrap();
    assert_eq!(a.await, 17);
    let a = first.take().request.unwrap();
    let b = second.take().request.unwrap();
    assert_eq!(
        (
            a.identity.resolutions,
            a.identity.resolution_us,
            a.identity.provider_dispatches
        ),
        (1, 3000, 2)
    );
    assert_eq!(
        (
            b.identity.resolutions,
            b.identity.resolution_us,
            b.identity.provider_dispatches
        ),
        (1, 7000, 6)
    );
    let outer = outer.0.lock().unwrap();
    assert_eq!(
        (
            outer.resolutions,
            outer.resolution_us,
            outer.provider_dispatches
        ),
        (1, 11000, 0)
    );
}

#[tokio::test(start_paused = true)]
async fn wake_delay_excludes_unready_wait_and_does_not_restart_for_repeated_wakes() {
    let profile = Profile::default();
    let waker = Arc::new(Mutex::new(None::<Waker>));
    let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let inner_waker = waker.clone();
    let ready = completed.clone();
    let mut future = Box::pin(profile.measure(poll_fn(move |cx| {
        *inner_waker.lock().unwrap() = Some(cx.waker().clone());
        if ready.load(std::sync::atomic::Ordering::SeqCst) {
            Poll::Ready(42)
        } else {
            Poll::Pending
        }
    })));
    assert!(futures_util::poll!(future.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(10)).await;
    waker.lock().unwrap().as_ref().unwrap().wake_by_ref();
    tokio::time::advance(Duration::from_secs(4)).await;
    waker.lock().unwrap().as_ref().unwrap().wake_by_ref();
    tokio::time::advance(Duration::from_secs(3)).await;
    completed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(future.await, 42);
    let sample = profile.take().request.unwrap();
    assert!(sample.completed);
    assert_eq!(sample.wall_us, 17_000_000);
    assert_eq!(sample.wake_to_poll_us, 7_000_000);
    assert_eq!(sample.poll_us, 0);
    assert_eq!((sample.polls, sample.woken_polls), (2, 1));
}

struct Parent;
impl Wake for Parent {
    fn wake(self: Arc<Self>) {}
}

#[tokio::test]
async fn cancelling_records_the_real_identity_guard_drop_and_releases_the_parent_waker() {
    let profile = Profile::default();
    let retained_waker = Arc::new(Mutex::new(None::<Waker>));
    let inner_waker = retained_waker.clone();
    let mut future = Box::pin(profile.measure(async {
        let request = axum::extract::Request::new(axum::body::Body::empty());
        let _resolution = crate::telemetry::request::Resolution::start(&request, "ambient");
        poll_fn(move |cx| {
            *inner_waker.lock().unwrap() = Some(cx.waker().clone());
            Poll::<()>::Pending
        })
        .await;
    }));
    let parent = Arc::new(Parent);
    let weak = Arc::downgrade(&parent);
    let waker = Waker::from(parent);
    assert!(future
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending());
    drop(future);
    drop(waker);
    assert!(
        weak.upgrade().is_none(),
        "retained child waker must not retain its executor task"
    );
    retained_waker
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .wake_by_ref();
    let sample = profile.take().request.unwrap();
    assert!(!sample.completed);
    assert_eq!(sample.identity.resolutions, 1);
    assert_eq!(sample.identity.cancelled, 1);
}

#[tokio::test]
async fn real_resolution_and_inventory_spans_remain_bound_to_one_request() {
    use tracing::instrument::WithSubscriber;
    let profile = Profile::default();
    profile
        .measure(
            async {
                let request = axum::extract::Request::new(axum::body::Body::empty());
                let mut resolution =
                    crate::telemetry::request::Resolution::start(&request, "ambient");
                resolution.finish("resolved", crate::telemetry::request::Consumer::PublicUser);
                drop(resolution);
                tracing::debug_span!(target: "cma.agent_inventory", "sources").in_scope(|| {
                    std::hint::black_box(1 + 2);
                });
            }
            .with_subscriber(profile.dispatch()),
        )
        .await;
    let sample = profile.take();
    assert!(sample.phases.contains_key("sources"));
    assert_eq!(sample.request.as_ref().unwrap().identity.resolved, 1);
    report(&[sample]);
}

#[test]
fn invalid_histograms_and_unknown_labels_cannot_create_valid_identity_evidence() {
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        u64::MAX as f64,
    ] {
        let recorder = identity::Recorder::default();
        metrics::with_local_recorder(&recorder, || resolution(value));
        assert!(recorder.0.lock().unwrap().invalid);
    }
    let recorder = identity::Recorder::default();
    metrics::with_local_recorder(&recorder, || {
        metrics::histogram!("cma_auth_resolution_seconds", "path" => "private-path", "outcome" => "private-identity").record(0.01);
        metrics::counter!("unrelated-private-metric", "private" => "private-value").increment(4);
    });
    let sample = recorder.0.lock().unwrap();
    assert!(sample.invalid);
    assert_eq!(sample.provider_dispatches, 0);
    assert!(!serde_json::to_string(&*sample).unwrap().contains("private"));
}

#[tokio::test]
async fn reports_reject_missing_duplicate_failed_or_cancelled_resolution_evidence() {
    for mode in ["missing", "duplicate", "failed", "cancelled"] {
        let profile = Profile::default();
        profile.measure(async {
            match mode {
                "missing" => {},
                "duplicate" => { resolution(0.01); resolution(0.02); },
                "failed" => metrics::histogram!("cma_auth_resolution_seconds", "path" => "ambient", "outcome" => "unavailable").record(0.01),
                _ => metrics::histogram!("cma_auth_resolution_seconds", "path" => "ambient", "outcome" => "cancelled").record(0.01),
            }
        }).await;
        let sample = profile.take();
        assert!(
            std::panic::catch_unwind(|| report(&[sample])).is_err(),
            "{mode}"
        );
    }
}
