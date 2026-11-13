use super::*;

fn page(after: Option<u64>, messages: Value, answers: Value) -> Value {
    let mut value = json!({"agent_id":"agent", "through_seq":80,"turns":[{
        "turn_sequence":3,"started_at":NOW,"owner_message":"owner",
        "outcome":{"type":"completed","at":NOW},"messages":messages,"agent_messages":answers,
    }]});
    if let Some(after) = after {
        value["next"] = json!({"turn":3,"after":after,"through":80,"limit":32});
    }
    value
}
fn require_pages(server: &Server) {
    server.expect(
        "GET",
        "/agents/agent/transcript",
        None,
        400,
        Some(json!({"code":"transcript_pagination_required","detail":"Use pages"})),
    );
}
fn owner() -> Value {
    json!({"sequence":4,"role":"user","content":"owner","appended_at":NOW})
}
fn answer() -> Value {
    json!({"sequence":35,"role":"assistant","content":"answer","appended_at":NOW})
}

#[tokio::test]
async fn sdk_transcript_hydrates_long_turns_and_empty_source_pages_exactly() {
    let server = Server::new().await;
    require_pages(&server);
    server.expect(
        "GET",
        "/agents/agent/transcript?limit=32",
        None,
        200,
        Some(page(Some(34), json!([owner()]), json!([]))),
    );
    server.expect(
        "GET",
        "/agents/agent/transcript?limit=32&turn=3&after=34&through=80",
        None,
        200,
        Some(page(Some(66), json!([answer()]), json!(["answer"]))),
    );
    server.expect(
        "GET",
        "/agents/agent/transcript?limit=32&turn=3&after=66&through=80",
        None,
        200,
        Some(page(None, json!([]), json!([]))),
    );
    let result = server.client.agents().transcript("agent").await.unwrap();
    let expected: AgentTranscript =
        serde_json::from_value(page(None, json!([owner(), answer()]), json!(["answer"]))).unwrap();
    assert_eq!(result, expected);
    server.assert_drained();
}

#[tokio::test]
async fn sdk_transcript_rejects_snapshot_drift_repeated_cursors_and_messages() {
    for fault in ["snapshot", "cursor", "message", "metadata"] {
        let server = Server::new().await;
        require_pages(&server);
        server.expect(
            "GET",
            "/agents/agent/transcript?limit=32",
            None,
            200,
            Some(page(Some(34), json!([owner()]), json!([]))),
        );
        let mut bad = page(Some(66), json!([answer()]), json!(["answer"]));
        match fault {
            "snapshot" => bad["through_seq"] = 81.into(),
            "cursor" => bad["next"]["after"] = 34.into(),
            "metadata" => bad["turns"][0]["owner_message"] = "changed".into(),
            _ => {
                bad["turns"][0]["messages"] = json!([owner()]);
                bad["turns"][0]["agent_messages"] = json!([]);
            }
        }
        server.expect(
            "GET",
            "/agents/agent/transcript?limit=32&turn=3&after=34&through=80",
            None,
            200,
            Some(bad),
        );
        assert!(
            matches!(
                server.client.agents().transcript("agent").await,
                Err(Error::Protocol { .. })
            ),
            "{fault}"
        );
        server.assert_drained();
    }
}

#[tokio::test]
async fn sdk_transcript_propagates_revocation_and_validates_page_bounds() {
    let server = Server::new().await;
    for limit in [0, 33] {
        assert!(matches!(
            server
                .client
                .agents()
                .transcript_page("agent", limit, None)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    require_pages(&server);
    server.expect(
        "GET",
        "/agents/agent/transcript?limit=32",
        None,
        200,
        Some(page(Some(34), json!([owner()]), json!([]))),
    );
    server.expect(
        "GET",
        "/agents/agent/transcript?limit=32&turn=3&after=34&through=80",
        None,
        403,
        Some(json!({"code":"forbidden","detail":"revoked"})),
    );
    assert!(matches!(
        server.client.agents().transcript("agent").await,
        Err(Error::Api { status: 403, .. })
    ));
    server.assert_drained();
}
