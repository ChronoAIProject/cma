use super::*;
use crate::activity::ExecutionAttribution;

fn change(response: &str, reference: &str) -> Change {
    Change::execution_attribution(
        "default",
        ExecutionAttribution::new("cma_codex".into(), response.into(), reference.into()).unwrap(),
    )
}

#[tokio::test]
async fn execution_receipt_is_durable_and_only_projected_for_the_matching_owner_and_response() {
    let (server, mut state, request, mut creator) = fixture_app().await;
    let _auth = live_auth(&mut state, &mut creator).await;
    let record = request.channel.definition.record_id.as_str();
    let event = request.activity.event_id.as_str();
    let channel_id = request.channel.id.to_string();
    let original = state.activity.clone();
    state.activity.received(&request).await.unwrap();
    let mut accepted = Change::state("default", ConsumerState::Accepted, Some("r1".into()));
    accepted.provider = Some("cma_codex".into());
    accepted.agent_id = Some("provider-agent".into());
    state
        .activity
        .change(record, event, &accepted)
        .await
        .unwrap();
    state
        .activity
        .change(record, event, &change("r1", "disabled"))
        .await
        .unwrap();
    state.activity = Arc::new((*original).clone().with_execution_attribution(true));
    for receipt in [
        change("r1", "terminal-executor"),
        change("r1", "terminal-executor"),
        change("r1", "conflicting-executor"),
        change("r2", "different-response"),
    ] {
        state
            .activity
            .change(record, event, &receipt)
            .await
            .unwrap();
    }
    state.activity = Arc::new(
        ActivityStore::new(server.records.clone(), Arc::new(fixture::keys()))
            .with_execution_attribution(true),
    );
    let read = |state: ManagementState, creator: Creator| {
        let channel_id = channel_id.clone();
        async move {
            let response =
                crate::http::activity::latest(State(state), creator, Path(channel_id)).await;
            let body = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        }
    };
    let expected = read(state.clone(), creator.clone()).await;
    let consumer = &expected["items"][0]["consumers"][0];
    assert_eq!(consumer["agent_id"], "provider-agent");
    assert_eq!(consumer["execution_reference"], "terminal-executor");
    assert!(consumer.get("execution_attributions").is_none());
    let stream = |creator: Creator| {
        let channel_id = channel_id.clone();
        let state = state.clone();
        let cursor = expected["cursor"].as_str().unwrap().to_owned();
        async move {
            crate::http::activity::stream(
                State(state),
                creator,
                Path(channel_id),
                Query(crate::http::activity::ActivityQuery {
                    cursor: Some(cursor),
                }),
                auth_headers(),
            )
            .await
        }
    };
    let mut stream_body = stream(creator.clone()).await.into_body().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(2), stream_body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let data = std::str::from_utf8(&frame)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(data).unwrap(),
        expected
    );

    let mut foreign = creator.clone();
    foreign.subject = "another-owner".into();
    assert_eq!(
        stream(foreign.clone()).await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(read(state.clone(), foreign).await.get("items").is_none());

    let next = Change::state(
        "default",
        ConsumerState::Accepted,
        Some("unverified-response".into()),
    );
    state.activity.change(record, event, &next).await.unwrap();
    assert!(
        read(state.clone(), creator.clone()).await["items"][0]["consumers"][0]
            .get("execution_reference")
            .is_none()
    );
    state
        .activity
        .change(record, event, &accepted)
        .await
        .unwrap();
    state.activity = original;
    assert!(read(state, creator).await["items"][0]["consumers"][0]
        .get("execution_reference")
        .is_none());
}
