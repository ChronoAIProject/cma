use super::*;
use layer2_core::sandbox::{
    store::{AgentResponseLogStore, AgentResponseLogWindow, SandboxStoreError},
    Agent,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Source {
    calls: AtomicUsize,
    fail: AtomicBool,
    stall: AtomicBool,
}
#[async_trait]
impl AgentResponseLogStore for Source {
    async fn agent_response_log_window(
        &self,
        _: &Agent,
        _: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.stall.load(Ordering::SeqCst) {
            return std::future::pending().await;
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::Unavailable(
                "test source unavailable".into(),
            ));
        }
        Ok(None)
    }
}

#[tokio::test(start_paused = true)]
async fn headers_do_not_read_history_and_unchanged_polls_skip_source_projection() {
    let source = Arc::new(Source::default());
    let f = fixture_with_events(Some(source.clone()), true).await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(800), body.next())
            .await
            .is_err()
    );
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        1,
        "three idle polls must not scan again"
    );
    let command = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &command.receipt.envelope.command_id,
            command.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.failed"
    );
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        2,
        "command-only change must be observed"
    );
    assert!(body.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_stalled_source_cannot_delay_headers_or_change_the_command_outcome() {
    let source = Arc::new(Source::default());
    source.stall.store(true, Ordering::SeqCst);
    let f = fixture_with_events(Some(source.clone()), true).await;
    let response = tokio::time::timeout(Duration::from_millis(10), create(&f, false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    let mut body = response.into_body().into_data_stream();
    let bytes = body.next().await.unwrap().unwrap();
    assert_eq!(
        events(&bytes)[0]["code"],
        "response_observation_unavailable"
    );
    assert!(!String::from_utf8_lossy(&bytes)
        .lines()
        .any(|line| line.starts_with("id:")));
    assert!(body.next().await.is_none());
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items[0]
            .status,
        AgentResponseStatus::Queued
    );
}

#[tokio::test]
async fn source_failure_after_headers_has_no_replay_id_and_does_not_write() {
    let source = Arc::new(Source::default());
    source.fail.store(true, Ordering::SeqCst);
    let f = fixture_with_events(Some(source.clone()), true).await;
    let response = create(&f, false).await;
    let before = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    let bytes = axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    assert_eq!(
        events(&bytes)[0]["code"],
        "response_observation_unavailable"
    );
    assert!(!String::from_utf8_lossy(&bytes)
        .lines()
        .any(|line| line.starts_with("id:")));
    let after = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    assert_eq!(after, before);
}

#[tokio::test]
async fn future_cursor_is_a_connection_error_and_overflow_is_refused_before_headers() {
    let source = Arc::new(Source::default());
    let f = fixture_with_events(Some(source), true).await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events?starting_after=99"),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 10000)
        .await
        .unwrap();
    assert_eq!(events(&bytes)[0]["code"], "invalid_replay_cursor");
    assert!(!String::from_utf8_lossy(&bytes)
        .lines()
        .any(|line| line.starts_with("id:")));
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events?starting_after={}", u64::MAX),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn default_off_keeps_pre_header_observation() {
    let source = Arc::new(Source::default());
    source.fail.store(true, Ordering::SeqCst);
    let f = fixture_with_events(Some(source.clone()), false).await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}
