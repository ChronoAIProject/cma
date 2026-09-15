use super::*;

#[tokio::test]
async fn rejects_foreign_or_inconsistent_queued_source_observations() {
    let server = WireServer::new().await;
    for (path, value) in [
        ("/object", json!("foreign")),
        ("/command_revision", json!(0)),
        ("/command_revision", json!(u64::MAX)),
        ("/response/receipt/agent_id", json!("foreign")),
        ("/response/receipt/response_id", Value::Null),
        ("/response/status", json!("in_progress")),
        ("/response/turn_sequence", json!(4)),
        ("/response/failure", json!("queue_timeout")),
    ] {
        let mut body = observation();
        *body.pointer_mut(path).unwrap() = value;
        server.expect(wire::json("GET", ROUTE, None, 200, body, true));
        assert!(matches!(
            server
                .client
                .responses_v2()
                .observe_queued_cancellation(AGENT, RESPONSE)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    let mut inactive = observation();
    inactive["cancellable"] = json!(false);
    inactive["response"]["status"] = json!("in_progress");
    inactive["response"]["turn_sequence"] = json!(4);
    inactive["response"]["delivery"] = json!("dispatch_fenced");
    server.expect(wire::json("GET", ROUTE, None, 200, inactive, true));
    assert!(
        !server
            .client
            .responses_v2()
            .observe_queued_cancellation(AGENT, RESPONSE)
            .await
            .unwrap()
            .cancellable
    );
    server.assert_drained();
}

#[tokio::test]
async fn refuses_changed_target_or_cancellation_completion_and_requires_explicit_nulls() {
    let server = WireServer::new().await;
    for (path, value) in [
        ("/object", json!("foreign")),
        ("/target/response_id", json!("foreign")),
        ("/target/source_command_id", json!("foreign")),
        ("/target/expected_command_revision", json!(2)),
        ("/cancellation/receipt/agent_id", json!("foreign")),
        ("/cancellation/receipt/command_id", json!(SOURCE)),
        ("/cancellation/receipt/response_id", json!(RESPONSE)),
        ("/cancellation/status", json!("queued")),
        ("/cancellation/turn_sequence", json!(4)),
        ("/cancellation/delivery", json!("write_confirmed")),
        ("/cancellation/observed_cursor", json!(4)),
        ("/cancellation/failure", json!("queue_timeout")),
        ("/response/receipt/agent_id", json!("foreign")),
        ("/response/receipt/response_id", json!("foreign")),
        ("/response/receipt/command_id", json!("foreign")),
    ] {
        let mut body = receipt();
        *body.pointer_mut(path).unwrap() = value;
        server.expect(expected(body, 200, "saved"));
        assert!(matches!(
            server
                .client
                .responses_v2()
                .cancel_queued_response(AGENT, RESPONSE, &intent(), "saved")
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    for mode in ["missing-null", "unknown-field", "wrong-status"] {
        let mut body = receipt();
        if mode == "missing-null" {
            body["cancellation"]
                .as_object_mut()
                .unwrap()
                .remove("failure");
        }
        if mode == "unknown-field" {
            body["target"]["actor"] = json!("forged");
        }
        server.expect(expected(
            body,
            if mode == "wrong-status" { 202 } else { 200 },
            "saved",
        ));
        assert!(matches!(
            server
                .client
                .responses_v2()
                .cancel_queued_response(AGENT, RESPONSE, &intent(), "saved")
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[test]
fn discovery_extension_is_optional_and_strict() {
    use cma_sdk::responses_v2::Capabilities;
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../api/crates/layer1-public/tests/fixtures/public-contract/responses-v2.json"
    ))
    .unwrap();
    let mut body = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["operation"] == "GET /api/v2" && case["response"]["status"] == 200)
        .unwrap()["response"]["body"]
        .clone();
    body.as_object_mut().unwrap().remove("queued_cancellation");
    assert!(
        serde_json::from_value::<Capabilities>(body.clone())
            .unwrap()
            .queued_cancellation
            .is_none()
    );
    let capability = json!({"path":ROUTE,"observe":"GET","submit":"POST","idempotency_header":"Idempotency-Key","idempotency_required":true,"target":"unbound_response_command","completion":"source_cancelled"});
    body["queued_cancellation"] = capability.clone();
    assert!(
        serde_json::from_value::<Capabilities>(body.clone())
            .unwrap()
            .queued_cancellation
            .is_some()
    );
    for (key, value) in [
        ("idempotency_required", json!(false)),
        ("idempotency_header", json!("other")),
        ("target", json!("bound_response_turn")),
        ("completion", json!("command_delivery")),
        ("private", json!(true)),
    ] {
        body["queued_cancellation"] = capability.clone();
        body["queued_cancellation"][key] = value;
        assert!(serde_json::from_value::<Capabilities>(body.clone()).is_err());
    }
}
