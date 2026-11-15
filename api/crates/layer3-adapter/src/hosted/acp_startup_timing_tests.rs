use super::*;
use crate::hosted::startup_timing::tests::Capture;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Copy)]
enum Branch {
    New,
    Load,
    Rejoin,
    Prepared,
}

impl Branch {
    fn label(self) -> &'static str {
        match self {
            Self::New => "fresh_new",
            Self::Load => "fresh_load",
            Self::Rejoin => "existing_rejoin",
            Self::Prepared => "prepared_rejoin",
        }
    }

    fn operations(self) -> &'static [&'static str] {
        match self {
            Self::New => &["session_new", "activate_session"],
            Self::Load => &["activate_session", "session_load"],
            Self::Rejoin => &["activate_session"],
            Self::Prepared => &["session_close", "activate_session", "session_load"],
        }
    }

    fn methods(self) -> &'static [&'static str] {
        match self {
            Self::New => &["session/new"],
            Self::Load => &["session/load"],
            Self::Rejoin => &[],
            Self::Prepared => &["session/close", "session/load"],
        }
    }
}

#[tokio::test]
async fn actual_handshakes_attribute_all_session_branches_without_extra_calls() {
    for branch in [Branch::New, Branch::Load, Branch::Rejoin, Branch::Prepared] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            assert_eq!(initialize["method"], "initialize");
            if matches!(branch, Branch::Rejoin | Branch::Prepared) {
                reject(&mut server, &initialize).await;
            } else {
                respond(&mut server, &initialize, serde_json::json!({})).await;
            }
            for method in branch.methods() {
                let request = receive_json(&mut server).await;
                assert_eq!(request["method"], *method);
                if *method != "session/new" {
                    assert_eq!(request["params"]["sessionId"], "private-session");
                } else {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                respond(
                    &mut server,
                    &request,
                    serde_json::json!({"sessionId": "private-session"}),
                )
                .await;
            }
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "unexpected additional ACP call"
                );
            }
        });
        let (updates, _receiver) = tokio::sync::mpsc::channel(8);
        let mut configured = settings(None, false);
        if matches!(branch, Branch::Prepared) {
            configured.access_tool = Some(
                access_tool_tests::binding(None)
                    .with_prepared_conversation("private-session")
                    .unwrap(),
            );
        }
        let capture = Capture::default();
        let opened = Bridge::open(
            client,
            "agent-1",
            "/private-workspace".into(),
            configured,
            (!matches!(branch, Branch::New)).then(|| "private-session".into()),
            updates,
        )
        .with_subscriber(capture.subscriber())
        .await
        .unwrap();
        stop(opened).await;
        serving.await.unwrap();
        let records = capture.subphases();
        assert_eq!(
            records
                .iter()
                .map(|r| r.fields["operation"].as_str())
                .collect::<Vec<_>>(),
            branch.operations()
        );
        use sha2::{Digest, Sha256};
        let expected_digest = format!("{:x}", Sha256::digest(b"agent-1"));
        for row in &records {
            assert_eq!(row.fields["branch"], branch.label());
            assert_eq!(row.fields["outcome"], "ok");
            assert_eq!(row.fields["correlation_digest"], expected_digest);
            assert_eq!(row.fields["attempt_id"], records[0].fields["attempt_id"]);
            assert!(!row.contextual && !row.has_parent);
        }
        if matches!(branch, Branch::New) {
            assert!(records[0].fields["elapsed_ms"].parse::<u64>().unwrap() >= 25);
            let enclosing = capture
                .records()
                .into_iter()
                .find(|row| row.fields.get("phase").map(String::as_str) == Some("protocol_session"))
                .unwrap();
            assert!(enclosing.fields["elapsed_ms"].parse::<u64>().unwrap() >= 25);
            assert!(!enclosing.contextual && !enclosing.has_parent);
        }
        let text = format!("{records:?}");
        for private in ["private-session", "private-workspace", "agent-1", "Bearer"] {
            assert!(
                !text.contains(private),
                "unexpected private field {private}"
            );
        }
    }
}

#[tokio::test]
async fn rejected_session_rpc_records_error_without_activation_or_private_error_text() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let request = receive_json(&mut server).await;
        assert_eq!(request["method"], "session/new");
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0", "id": request["id"],
                "error": {"code": -32603, "message": "private-provider-failure"},
            }),
        )
        .await;
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = tokio::sync::mpsc::channel(8);
    let capture = Capture::default();
    let result = Bridge::open(
        client,
        "agent-1",
        "/private-workspace".into(),
        settings(None, false),
        None,
        updates,
    )
    .with_subscriber(capture.subscriber())
    .await;
    assert!(result.is_err());
    serving.await.unwrap();
    let records = capture.subphases();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].fields["operation"], "session_new");
    assert_eq!(records[0].fields["outcome"], "error");
    assert!(!format!("{records:?}").contains("private-provider-failure"));
}
