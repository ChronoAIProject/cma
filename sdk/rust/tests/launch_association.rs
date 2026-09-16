#[allow(dead_code)]
#[path = "support/wire.rs"]
mod wire;

use serde_json::{Value, json};
use wire::{Expected, WireServer};

fn launch() -> Value {
    json!({"id":"lch_original","trigger_id":"trg_original","revision_digest":"original",
        "public_id":"pub_original","state":"draft","steps":[],
        "resolution":{"sandbox":{"state":"pending"},"workspace":{"state":"pending"},"agent":{"state":"pending"},"choice_required":false},
        "requirements":{"services":[],"checked_at":"2026-09-16T00:00:00Z"},
        "review":{"needs":[],"can_start_now":true},"trust":{"version":1,"disclosures":[]},
        "first_message":"queued","confirmations":[],"event_head":1,"created_at":"2026-09-16T00:00:00Z","updated_at":"2026-09-16T00:00:00Z","revision":1})
}

fn read(body: Value) -> Expected {
    Expected {
        method: "GET".into(),
        path: "/api/v1/launches/lch_original".into(),
        headers: [
            ("authorization".into(), "Bearer sdk-fixture".into()),
            ("accept".into(), "application/json".into()),
        ]
        .into(),
        status: 200,
        response: serde_json::to_vec(&body).unwrap(),
        ..Default::default()
    }
}

#[tokio::test]
async fn launch_read_retains_immutable_association_and_legacy_omission_without_resubmission() {
    let server = WireServer::new().await;
    let mut current = launch();
    current["first_message_command"] =
        json!({"agent_id":"agt_original","command_id":"cmd_original","response_id":"rsp_original"});
    server.expect(read(current));
    let association = server
        .client
        .launches()
        .read("lch_original")
        .await
        .unwrap()
        .first_message_command
        .unwrap();
    assert_eq!(association.agent_id, "agt_original");
    assert_eq!(association.command_id, "cmd_original");
    assert_eq!(association.response_id, "rsp_original");
    server.expect(read(launch()));
    assert!(
        server
            .client
            .launches()
            .read("lch_original")
            .await
            .unwrap()
            .first_message_command
            .is_none()
    );
    let mut malformed = launch();
    malformed["first_message_command"] =
        json!({"agent_id":"agt_original","command_id":"cmd_original"});
    server.expect(read(malformed));
    assert!(server.client.launches().read("lch_original").await.is_err());
    assert_eq!(server.request_count(), 3);
    server.assert_drained();
}
