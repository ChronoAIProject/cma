#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, IdempotencyKey, agents::*};
use serde_json::{Value, json};
use wire::WireServer;

fn receipt(status: &str) -> Value {
    let mut value = json!({"id":"access-one","agent_id":"agent-one","status":status,
        "services":["api-github"],"reason":"Read the selected repository."});
    if status == "connecting" {
        value["connection"] = json!({"target":{"type":"agent_access_request","id":"access-one"}});
    }
    value
}

#[tokio::test]
async fn access_commands_preserve_exact_identity_body_key_and_current_progress() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("exact-decision").unwrap();
    let body = ApproveAgentAccessRequest {
        services: vec!["api-github".into()],
    };
    server.expect(wire::json(
        "GET",
        "/api/v1/agents/agent-one/access-requests",
        None,
        200,
        json!({"items":[receipt("pending")]}),
        true,
    ));
    assert_eq!(
        server
            .client
            .agents()
            .access_requests("agent-one")
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    for status in ["connecting", "succeeded"] {
        let mut expected = wire::json(
            "POST",
            "/api/v1/agents/agent-one/access-requests/access-one/approve",
            Some(json!({"services":["api-github"]})),
            200,
            receipt(status),
            true,
        );
        expected
            .headers
            .insert("idempotency-key".into(), "exact-decision".into());
        server.expect(expected);
        let result = server
            .client
            .agents()
            .approve_access_request("agent-one", "access-one", &body, &key)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(result).unwrap(), receipt(status));
    }
    let mut expected = wire::json(
        "POST",
        "/api/v1/agents/agent-one/access-requests/access-one/deny",
        Some(json!({})),
        200,
        receipt("denied"),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "exact-decision".into());
    server.expect(expected);
    assert_eq!(
        server
            .client
            .agents()
            .deny_access_request("agent-one", "access-one", &key)
            .await
            .unwrap()
            .status,
        AgentAccessStatus::Denied
    );
    server.assert_drained();
}

#[tokio::test]
async fn access_responses_reject_crossed_identity_incomplete_connections_and_duplicate_requests() {
    let server = WireServer::new().await;
    for field in ["agent_id", "id", "connection", "services", "status"] {
        let mut response = receipt("connecting");
        match field {
            "agent_id" => response["agent_id"] = json!("other-agent"),
            "id" => response["id"] = json!("other-request"),
            "connection" => {
                response.as_object_mut().unwrap().remove("connection");
            }
            "services" => response["services"] = json!(["bad service"]),
            "status" => response["status"] = json!("invented"),
            _ => unreachable!(),
        }
        let mut expected = wire::json(
            "POST",
            "/api/v1/agents/agent-one/access-requests/access-one/approve",
            Some(json!({"services":["api-github"]})),
            200,
            response,
            true,
        );
        expected
            .headers
            .insert("idempotency-key".into(), "decision".into());
        server.expect(expected);
        let error = server
            .client
            .agents()
            .approve_access_request(
                "agent-one",
                "access-one",
                &ApproveAgentAccessRequest {
                    services: vec!["api-github".into()],
                },
                &IdempotencyKey::new("decision").unwrap(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::Protocol { .. }),
            "{field}: {error:?}"
        );
    }
    server.expect(wire::json(
        "GET",
        "/api/v1/agents/agent-one/access-requests",
        None,
        200,
        json!({"items":[receipt("pending"),receipt("pending")]}),
        true,
    ));
    assert!(matches!(
        server
            .client
            .agents()
            .access_requests("agent-one")
            .await
            .unwrap_err(),
        Error::Protocol { .. }
    ));
    server.assert_drained();
}

#[tokio::test]
async fn invalid_access_decisions_fail_before_network_effects() {
    let server = WireServer::new().await;
    for services in [
        vec![],
        vec!["x".repeat(129)],
        vec!["x".into(); 33],
        vec!["with space".into()],
        vec!["非ascii".into()],
    ] {
        assert!(matches!(
            server
                .client
                .agents()
                .approve_access_request(
                    "agent-one",
                    "access-one",
                    &ApproveAgentAccessRequest { services },
                    &IdempotencyKey::new("decision").unwrap()
                )
                .await
                .unwrap_err(),
            Error::Validation { .. }
        ));
    }
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
}

#[tokio::test]
async fn failed_predecessor_identity_survives_a_fresh_pending_consent_attempt() {
    let server = WireServer::new().await;
    let prior = format!("aar_{}", "a".repeat(64));
    let current = format!("aar_{}", "b".repeat(64));
    let mut next = receipt("pending");
    next["id"] = current.clone().into();
    next["retry_of"] = prior.clone().into();
    server.expect(wire::json(
        "GET",
        "/api/v1/agents/agent-one/access-requests",
        None,
        200,
        json!({"items":[next.clone()]}),
        true,
    ));
    let result = server
        .client
        .agents()
        .access_requests("agent-one")
        .await
        .unwrap();
    assert_eq!(result.items[0].retry_of.as_deref(), Some(prior.as_str()));
    assert_eq!(serde_json::to_value(&result.items[0]).unwrap(), next);
    for invalid in ["foreign".to_owned(), current] {
        next["retry_of"] = invalid.into();
        server.expect(wire::json(
            "GET",
            "/api/v1/agents/agent-one/access-requests",
            None,
            200,
            json!({"items":[next.clone()]}),
            true,
        ));
        assert!(matches!(
            server.client.agents().access_requests("agent-one").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}
