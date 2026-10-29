#[path = "provider_recovery/nyxbot.rs"]
mod nyxbot;
#[path = "provider_recovery/support.rs"]
mod support;
use cmaeg::providers::{ProviderError, ResponsesEvent};
use serde_json::json;
use std::time::Duration;
use support::*;

#[tokio::test]
async fn default_none_never_probes_or_reposts() {
    let server = Server::new(Config::default()).await;
    let client = server.client(false, Duration::from_secs(1));
    assert!(!client.supports_response_replay(&provider()).unwrap());
    assert!(matches!(
        client
            .response_snapshot(&provider(), &identity(), &key())
            .await,
        Err(ProviderError::InvalidRequest(_))
    ));
    assert!(matches!(
        client
            .replay_response(&provider(), &identity(), None, &key())
            .await,
        Err(ProviderError::InvalidRequest(_))
    ));
    assert!(server.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn snapshot_pins_native_agent_and_checks_journal_identity_and_exact_metadata() {
    let server = Server::new(Config::default()).await;
    let client = server.client(true, Duration::from_secs(1));
    let response = client
        .response_snapshot(&provider(), &identity(), &key())
        .await
        .unwrap();
    assert_eq!(response.native_agent_id, "agt_native");
    let mut pinned = identity();
    pinned.native_agent_id = Some(response.native_agent_id);
    client
        .response_snapshot(&provider(), &pinned, &key())
        .await
        .unwrap();
    // Gateway conversation aliases are not the native CMA Agent ID.
    pinned.native_agent_id = Some("conv_gateway_alias".into());
    assert!(matches!(
        client.response_snapshot(&provider(), &pinned, &key()).await,
        Err(ProviderError::ResponseMalformed(_))
    ));
    for (pointer, value) in [
        ("/id", json!("resp_other")),
        ("/metadata/cmaeg_event_id", json!("another_event")),
        ("/conversation/id", json!(null)),
    ] {
        let mut altered = snapshot("completed");
        *altered.pointer_mut(pointer).unwrap() = value;
        server.config.lock().unwrap().snapshot = altered;
        assert!(matches!(
            client
                .response_snapshot(&provider(), &identity(), &key())
                .await,
            Err(ProviderError::ResponseMalformed(_))
        ));
    }
    let seen = server.seen.lock().unwrap();
    assert!(seen.iter().all(|request| request.method == "GET"
        && request.uri == "/api/v2/responses/resp_accepted"
        && request.authorization == "Bearer fixture-current-agent-key"));
}

#[tokio::test]
async fn replay_retains_contiguous_durable_ids_response_identity_and_cursor() {
    let server = Server::new(Config::default()).await;
    let client = server.client(true, Duration::from_secs(1));
    let mut stream = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    let mut observed = Vec::new();
    while let Some(event) = stream.events.recv().await {
        observed.push(event.unwrap());
    }
    assert_eq!(
        observed
            .iter()
            .map(|event| event.sequence_number)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert!(observed
        .iter()
        .all(|event| event.response_id == "resp_accepted"
            && event.raw["sequence_number"] == event.sequence_number));
    assert!(matches!(
        observed[2].event,
        ResponsesEvent::OutputTextDelta { .. }
    ));
    server.config.lock().unwrap().frames = frame(
        Some("2"),
        json!({"type":"response.output_text.delta","sequence_number":2,"item_id":"msg_1","delta":"hello"}),
    ) + &lifecycle(3, "response.completed", "completed");
    let mut resumed = client
        .replay_response(&provider(), &identity(), Some(1), &key())
        .await
        .unwrap();
    assert_eq!(
        resumed
            .events
            .recv()
            .await
            .unwrap()
            .unwrap()
            .sequence_number,
        2
    );
    assert_eq!(
        resumed
            .events
            .recv()
            .await
            .unwrap()
            .unwrap()
            .sequence_number,
        3
    );
    assert!(resumed.events.recv().await.is_none());
    assert_eq!(
        server.seen.lock().unwrap().last().unwrap().uri,
        "/api/v2/responses/resp_accepted/events?starting_after=1"
    );
    assert!(server
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn observation_error_never_becomes_a_durable_failure_or_advances_cursor() {
    let server=Server::new(Config {frames:lifecycle(0,"response.created","queued")+&frame(None,json!({"type":"error","sequence_number":999,"code":"response_observation_unavailable"})),..Default::default()}).await;
    let client = server.client(true, Duration::from_secs(1));
    let mut stream = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    assert_eq!(
        stream.events.recv().await.unwrap().unwrap().sequence_number,
        0
    );
    assert!(matches!(
        stream.events.recv().await.unwrap(),
        Err(ProviderError::Unavailable(_))
    ));
    assert!(stream.events.recv().await.is_none());
}

#[tokio::test]
async fn replay_refuses_gaps_duplicate_or_missing_ids_forged_identity_and_false_terminal() {
    let server = Server::new(Config::default()).await;
    let client = server.client(true, Duration::from_secs(1));
    let first =
        json!({"type":"response.created","sequence_number":0,"response":snapshot("queued")});
    let mut wrong_response = first.clone();
    wrong_response["response"]["id"] = "resp_other".into();
    let mut wrong_metadata = first.clone();
    wrong_metadata["response"]["metadata"] = json!({});
    let mut wrong_agent = first.clone();
    wrong_agent["response"]["conversation"]["id"] = "agt_other".into();
    let mut pinned = identity();
    pinned.native_agent_id = Some("agt_native".into());
    let bad = [
        frame(Some("1"), first.clone()),
        frame(Some("00"), first.clone()),
        frame(None, first.clone()),
        frame(Some("0"), wrong_response),
        frame(Some("0"), wrong_metadata),
        frame(Some("0"), wrong_agent),
        frame(
            Some("0"),
            json!({"type":"response.completed","sequence_number":0}),
        ),
        frame(Some("0"), json!({"type":"error","sequence_number":0})),
        frame(Some("0"), first.clone())
            .replace("event: response.created", "event: response.completed"),
    ];
    for frames in bad {
        server.config.lock().unwrap().frames = frames;
        let mut stream = client
            .replay_response(&provider(), &pinned, None, &key())
            .await
            .unwrap();
        assert!(matches!(
            stream.events.recv().await.unwrap(),
            Err(ProviderError::ResponseMalformed(_))
        ));
        assert!(stream.events.recv().await.is_none());
    }
    for second in [
        lifecycle(0, "response.created", "queued"),
        lifecycle(2, "response.in_progress", "in_progress"),
    ] {
        server.config.lock().unwrap().frames = lifecycle(0, "response.created", "queued") + &second;
        let mut stream = client
            .replay_response(&provider(), &pinned, None, &key())
            .await
            .unwrap();
        assert!(stream.events.recv().await.unwrap().is_ok());
        assert!(matches!(
            stream.events.recv().await.unwrap(),
            Err(ProviderError::ResponseMalformed(_))
        ));
    }
}

#[tokio::test]
async fn replay_requires_exact_response_header_and_never_follows_redirects() {
    let redirect = Server::new(Config::default()).await;
    let server = Server::new(Config::default()).await;
    let client = server.client(true, Duration::from_secs(1));
    for header in [None, Some("resp_other".into())] {
        server.config.lock().unwrap().header = header;
        assert!(matches!(
            client
                .replay_response(&provider(), &identity(), None, &key())
                .await,
            Err(ProviderError::ResponseMalformed(_))
        ));
    }
    {
        let mut config = server.config.lock().unwrap();
        config.status = 307;
        config.location = Some(redirect.base.clone());
    }
    assert!(matches!(
        client
            .response_snapshot(&provider(), &identity(), &key())
            .await,
        Err(ProviderError::Rejected { status: 307, .. })
    ));
    assert!(redirect.seen.lock().unwrap().is_empty());
    assert!(server
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn http_errors_preserve_status_and_rate_limit_delay() {
    let server = Server::new(Config::default()).await;
    let client = server.client(true, Duration::from_secs(1));
    for status in [401, 403, 404, 409, 503] {
        server.config.lock().unwrap().status = status;
        let result = client
            .response_snapshot(&provider(), &identity(), &key())
            .await;
        assert!(
            matches!(result,Err(ProviderError::Rejected{status:actual,ref code}) if actual==status && code=="authority_unavailable")
        );
    }
    server.config.lock().unwrap().status = 429;
    assert!(
        matches!(client.replay_response(&provider(),&identity(),None,&key()).await,Err(ProviderError::Busy{retry_after:Some(delay)}) if delay==Duration::from_secs(2))
    );
}

#[tokio::test]
async fn idle_drop_and_deadline_release_the_only_provider_permit() {
    let server = Server::new(Config {
        idle: true,
        ..Default::default()
    })
    .await;
    let client = server.client(true, Duration::from_millis(120));
    let idle = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    drop(idle);
    tokio::time::timeout(
        Duration::from_millis(100),
        client.response_snapshot(&provider(), &identity(), &key()),
    )
    .await
    .unwrap()
    .unwrap();
    let mut idle = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), idle.events.recv())
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    client
        .response_snapshot(&provider(), &identity(), &key())
        .await
        .unwrap();
}

#[tokio::test]
async fn cancelling_header_wait_releases_capacity_without_submitting_a_turn() {
    let server = Server::new(Config {
        header_delay: Duration::from_secs(5),
        ..Default::default()
    })
    .await;
    let client = server.client(true, Duration::from_secs(1));
    assert!(tokio::time::timeout(
        Duration::from_millis(40),
        client.replay_response(&provider(), &identity(), None, &key())
    )
    .await
    .is_err());
    server.config.lock().unwrap().header_delay = Duration::ZERO;
    client
        .response_snapshot(&provider(), &identity(), &key())
        .await
        .unwrap();
    assert!(server
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.method == "GET"));
}

#[tokio::test]
async fn snapshot_and_replay_are_byte_bounded_and_truncated_stream_is_observation_failure() {
    let server = Server::new(Config::default()).await;
    let client = server.client(true, Duration::from_secs(1));
    server.config.lock().unwrap().snapshot["padding"] = json!("x".repeat(4 * 1024 * 1024));
    assert!(matches!(
        client
            .response_snapshot(&provider(), &identity(), &key())
            .await,
        Err(ProviderError::ResponseMalformed(_))
    ));
    server.config.lock().unwrap().frames = format!("data: {}\n\n", "x".repeat(1024 * 1024 + 1));
    let mut stream = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    assert!(stream.events.recv().await.unwrap().is_err());
    server.config.lock().unwrap().frames = lifecycle(0, "response.created", "queued");
    let mut stream = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    assert!(stream.events.recv().await.unwrap().is_ok());
    assert!(matches!(
        stream.events.recv().await.unwrap(),
        Err(ProviderError::Sse(_))
    ));
}

#[tokio::test]
async fn replay_wire_limit_counts_keepalive_bytes_without_advancing_a_cursor() {
    let server = Server::new(Config {
        frames: format!(": {}\n\n", "x".repeat(64 * 1024)).repeat(257),
        ..Default::default()
    })
    .await;
    let client = server.client(true, Duration::from_secs(3));
    let mut stream = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    assert!(
        matches!(stream.events.recv().await.unwrap(), Err(ProviderError::Sse(detail)) if detail.contains("byte limit"))
    );
    assert!(stream.events.recv().await.is_none());
    client
        .response_snapshot(&provider(), &identity(), &key())
        .await
        .unwrap();
}
