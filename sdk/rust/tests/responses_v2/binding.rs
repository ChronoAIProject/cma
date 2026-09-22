use super::{fixture, wire};
use cma_sdk::{
    Error,
    responses_v2::{CreateResponseRequest, EventContext},
};
use serde_json::json;
use wire::{Expected, WireServer, headers};

fn context() -> EventContext {
    serde_json::from_value(json!({"schema_version":1,"channel_id":"channel-independent","conversation_id":"external-conversation","event_ref":"opaque-reference","activity":{"schema_version":1,"event_id":"event-1","nested":{"full":"preserved"}}})).unwrap()
}
#[tokio::test]
async fn external_aliases_send_the_complete_context_and_accept_the_mapped_agent() {
    let server = WireServer::new().await;
    let mut request = CreateResponseRequest::new("external-conversation", "Hello");
    request.event_context = Some(context());
    for path in ["/api/v2/responses", "/api/v2/agents/agt_fixture/responses"] {
        server.expect(fixture::expected(
            path,
            Some(serde_json::to_value(&request).unwrap()),
            fixture::wire(&fixture::events()),
        ));
        let mut stream = if path == "/api/v2/responses" {
            server
                .client
                .responses_v2()
                .create(&request, None)
                .await
                .unwrap()
        } else {
            server
                .client
                .responses_v2()
                .create_for_agent("agt_fixture", &request, None)
                .await
                .unwrap()
        };
        let mut count = 0;
        while let Some(event) = stream.next().await {
            event.unwrap();
            count += 1;
        }
        assert_eq!(count, 9);
    }
    server.assert_drained();
}
#[tokio::test]
async fn context_reads_retain_nested_data_and_refuse_mismatched_identity() {
    let server = WireServer::new().await;
    let expected = context();
    for change in [None, Some("conversation_id"), Some("event_id")] {
        let mut value = serde_json::to_value(&expected).unwrap();
        match change {
            Some("event_id") => value["activity"]["event_id"] = "foreign".into(),
            Some(_) => value["conversation_id"] = "foreign".into(),
            None => {}
        }
        server.expect(Expected {
            method:"GET".into(), path:"/api/v1/cmaeg/bindings/binding-id/conversations/external-conversation/events/event-1".into(),
            headers:headers(true,"application/json"), status:200,
            response_headers: [("content-type".into(), "application/json".into())].into(),
            response:serde_json::to_vec(&value).unwrap(), ..Default::default()
        });
        let result = server
            .client
            .responses_v2()
            .read_event_context("binding-id", "external-conversation", "event-1")
            .await;
        if change.is_none() {
            assert_eq!(result.unwrap(), expected);
        } else {
            assert!(matches!(result, Err(Error::Protocol { .. })));
        }
    }
    server.assert_drained();
}
