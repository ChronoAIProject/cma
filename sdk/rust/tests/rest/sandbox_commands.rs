use super::*;

fn queued() -> Value {
    json!({"command_id":"scmd_one","sandbox_id":"sandbox-1","command":"pause","expected_revision":7,"admitted_at":NOW,"queue_deadline":NOW,"revision":1,"state":{"status":"queued"}})
}
fn request() -> SandboxCommandRequest {
    SandboxCommandRequest {
        command: SandboxCommandKind::Pause,
        expected_revision: 7,
    }
}

#[tokio::test]
async fn sandbox_commands_recover_queued_and_terminal_receipts_with_original_intent() {
    let server = Server::new().await;
    for state in [
        json!({"status":"queued"}),
        json!({"status":"applied","at":NOW,"sandbox_revision":8,"disposition":"scheduled"}),
        json!({"status":"applied","at":NOW,"sandbox_revision":7,"disposition":"already_scheduled"}),
        json!({"status":"applied","at":NOW,"sandbox_revision":7,"disposition":"already_satisfied"}),
        json!({"status":"rejected","at":NOW,"reason":"authority_revoked"}),
    ] {
        let mut receipt = queued();
        receipt["revision"] = json!(if state["status"] == "queued" { 1 } else { 2 });
        receipt["state"] = state;
        server.expect(
            "POST",
            "/sandboxes/sandbox-1/commands",
            Some(serde_json::to_value(request()).unwrap()),
            202,
            Some(receipt.clone()),
        );
        let accepted = server
            .client
            .sandboxes()
            .submit_command("sandbox-1", &request(), &key())
            .await
            .unwrap();
        server.expect(
            "GET",
            "/sandbox-commands/scmd_one",
            None,
            200,
            Some(receipt),
        );
        assert_eq!(
            server
                .client
                .sandboxes()
                .read_command("scmd_one")
                .await
                .unwrap(),
            accepted
        );
    }
    server.assert_drained();
}

#[tokio::test]
async fn sandbox_commands_refuse_malformed_and_mismatched_receipts() {
    let server = Server::new().await;
    for patch in [
        json!({"command_id":""}),
        json!({"sandbox_id":""}),
        json!({"sandbox_id":"another"}),
        json!({"command":"retire"}),
        json!({"command":"future"}),
        json!({"expected_revision":8}),
        json!({"expected_revision":0}),
        json!({"revision":2}),
        json!({"state":{"status":"future"}}),
        json!({"revision":2,"state":{"status":"applied","at":NOW,"sandbox_revision":7,"disposition":"scheduled"}}),
        json!({"revision":2,"state":{"status":"applied","at":NOW,"sandbox_revision":8,"disposition":"already_satisfied"}}),
        json!({"revision":2,"state":{"status":"rejected","at":NOW,"reason":"unknown"}}),
        json!({"revision":2,"state":{"status":"rejected","reason":"queue_expired"}}),
    ] {
        let mut receipt = queued();
        receipt
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        server.expect(
            "POST",
            "/sandboxes/sandbox-1/commands",
            Some(serde_json::to_value(request()).unwrap()),
            202,
            Some(receipt),
        );
        assert!(matches!(
            server
                .client
                .sandboxes()
                .submit_command("sandbox-1", &request(), &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    let mut receipt = queued();
    receipt["command_id"] = json!("scmd_another");
    server.expect(
        "GET",
        "/sandbox-commands/scmd_one",
        None,
        200,
        Some(receipt),
    );
    assert!(matches!(
        server.client.sandboxes().read_command("scmd_one").await,
        Err(Error::Protocol { .. })
    ));
    server.expect(
        "POST",
        "/sandboxes/sandbox-1/commands",
        Some(serde_json::to_value(request()).unwrap()),
        200,
        Some(queued()),
    );
    assert!(matches!(
        server
            .client
            .sandboxes()
            .submit_command("sandbox-1", &request(), &key())
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn sandbox_commands_validate_revision_before_http_and_refuse_unknown_request_fields() {
    let server = Server::new().await;
    for expected_revision in [0, i64::MAX as u64 + 1] {
        let body = SandboxCommandRequest {
            expected_revision,
            ..request()
        };
        assert!(matches!(
            server
                .client
                .sandboxes()
                .submit_command("sandbox-1", &body, &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(
        serde_json::from_value::<SandboxCommandRequest>(
            json!({"command":"pause","expected_revision":7,"owner":"injected"})
        )
        .is_err()
    );
    server.assert_drained();
}
