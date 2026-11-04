#[path = "support/wire.rs"]
mod wire;
use cma_sdk::answer_context::AnswerContext;
use serde_json::json;
use std::collections::BTreeMap;
use wire::{Expected, WireServer};

#[tokio::test]
async fn saved_suggestions_use_only_a_scoped_get_and_refuse_a_different_answer() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    let mut body = json!({"status":"ready","observation":null,"set":{
        "id":"saved","viewerId":"owner","agentId":"agt_fixture","answerId":"event:5",
        "scope":{"turn_sequence":3,"answer_sequence":5},"source":{"kind":"generated","version":"contextual-v3"},
        "items":[{"id":"saved:0","text":"Explain SQLite backup options.","rank":0}]
    }});
    for matches in [true, false] {
        if !matches {
            body["set"]["answerId"] = json!("event:9");
        }
        server.expect(Expected {
            method: "GET".into(),
            path: "/api/v1/agents/agt_fixture/suggestions?turn_sequence=3&answer_sequence=5".into(),
            headers: wire::headers(true, "application/json"),
            status: 200,
            response_headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            response: serde_json::to_vec(&body).unwrap(),
            ..Default::default()
        });
        let result = server
            .client
            .agents()
            .suggestions(
                "agt_fixture",
                AnswerContext {
                    turn_sequence: 3,
                    answer_sequence: 5,
                },
            )
            .await;
        if matches {
            assert_eq!(
                result.unwrap().set.unwrap().items[0].text,
                "Explain SQLite backup options."
            );
        } else {
            assert!(result.is_err());
        }
    }
    server.assert_drained();
    assert_eq!(server.request_count(), 2);
}
