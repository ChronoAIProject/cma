use super::protected_outage_tests::{application, credentials, request, READS};
use super::*;
use axum::http::Method;
use std::future::{poll_fn, Future};

pub(super) async fn measured<F: Future>(future: F) -> (F::Output, String) {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    tokio::pin!(future);
    let output =
        poll_fn(|cx| metrics::with_local_recorder(&recorder, || future.as_mut().poll(cx))).await;
    (output, handle.render())
}

pub(super) fn dispatches(output: &str) -> u64 {
    output
        .lines()
        .filter(|line| line.starts_with("cma_identity_provider_calls_total{"))
        .map(|line| line.rsplit_once(' ').unwrap().1.parse::<u64>().unwrap())
        .sum()
}

async fn reached<F: Future>(pending: std::pin::Pin<&mut F>, gates: &[&Arc<Gate>]) {
    tokio::select! {
        _ = pending => panic!("request completed before provider barriers"),
        result = tokio::time::timeout(Duration::from_secs(2), async {
            for gate in gates {
                gate.reached.notified().await;
            }
        }) => result.expect("provider requests must reach their barriers"),
    }
}

fn assert_refused(response: (StatusCode, HeaderMap, Value)) {
    let (status, headers, body) = response;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "store_failed");
    assert_eq!(body["retryable"], false);
    assert!(!headers.contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn late_cancelled_arrival_cannot_inflate_the_next_requests_dispatch_budget() {
    for cookie in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        fixture.auth.metadata().await.unwrap();
        let credentials = credentials(&fixture, cookie).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let app = application(fixture.auth.clone(), calls.clone());
        let first_userinfo = Arc::new(Gate::default());
        let late_arrival = Arc::new(Gate::default());
        {
            let mut reply = fixture.control.userinfo.write().await;
            reply.body = "{".into();
            reply.gate = Some(first_userinfo.clone());
        }
        *fixture.control.introspection_arrival.write().await = Some(late_arrival.clone());
        let first = measured(request(&app, &credentials, Method::GET, READS[0]));
        tokio::pin!(first);
        reached(first.as_mut(), &[&first_userinfo, &late_arrival]).await;
        first_userinfo.release.notify_one();
        let (response, first_metrics) = first.await;
        assert_refused(response);
        assert_eq!(dispatches(&first_metrics), 2, "{first_metrics}");
        assert!(
            first_metrics.contains("outcome=\"cancelled\""),
            "{first_metrics}"
        );

        let before = fixture.control.requests.load(Ordering::SeqCst);
        let counted = Arc::new(Gate::default());
        fixture.control.introspection.write().await.gate = Some(counted.clone());
        late_arrival.release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), counted.reached.notified())
            .await
            .expect("the already-dispatched request must reach the server counter");
        counted.release.notify_one();
        *fixture.control.introspection_arrival.write().await = None;

        let next_userinfo = Arc::new(Gate::default());
        let next_introspection = Arc::new(Gate::default());
        fixture.control.userinfo.write().await.gate = Some(next_userinfo.clone());
        fixture.control.introspection.write().await.gate = Some(next_introspection.clone());
        let second = measured(request(&app, &credentials, Method::GET, READS[1]));
        tokio::pin!(second);
        reached(second.as_mut(), &[&next_userinfo, &next_introspection]).await;
        next_userinfo.release.notify_one();
        next_introspection.release.notify_one();
        let (response, second_metrics) = second.await;
        assert_refused(response);
        let received = fixture.control.requests.load(Ordering::SeqCst) - before;
        assert_eq!(received, 3, "one late arrival plus two current arrivals");
        assert_eq!(dispatches(&second_metrics), 2, "{second_metrics}");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        eprintln!("cookie={cookie} prior_dispatches=2 current_dispatches=2 shared_arrival_delta={received}");
    }
}

#[tokio::test]
async fn dispatch_accounting_includes_every_real_retry_attempt() {
    for cookie in [false, true] {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        fixture.auth.metadata().await.unwrap();
        let credentials = credentials(&fixture, cookie).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let app = application(fixture.auth.clone(), calls.clone());
        fixture.control.userinfo.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
        let ((status, headers, body), metrics) =
            measured(request(&app, &credentials, Method::GET, READS[0])).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "identity_unavailable");
        assert_eq!(body["retryable"], true);
        assert!(headers.contains_key(header::RETRY_AFTER));
        assert_eq!(
            dispatches(&metrics),
            6,
            "three attempts, two endpoints: {metrics}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
