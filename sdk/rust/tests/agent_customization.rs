#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{Error, IdempotencyKey, agents::InstallAgentSkillRequest};
use serde_json::json;
use wire::{WireServer, json as response};

#[tokio::test]
async fn skill_installation_rejects_invalid_references_without_http() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("install-skill").unwrap();
    for id in [
        "".into(),
        ".hidden".into(),
        "research/path".into(),
        "research skill".into(),
        "nyxid".into(),
        "ornn-agent-manual-cli".into(),
        "研究".into(),
        "x".repeat(65),
    ] {
        let body = InstallAgentSkillRequest {
            id,
            version: "1.0".into(),
        };
        assert!(
            matches!(
                server
                    .client
                    .agents()
                    .install_skill("agt_one", &body, &key)
                    .await,
                Err(Error::Validation { .. })
            ),
            "invalid id: {:?}",
            body.id
        );
    }
    for version in [
        "".into(),
        "@latest".into(),
        " 1.0".into(),
        "1.0 ".into(),
        "1.0\n".into(),
        "1\u{0085}0".into(),
        "x".repeat(257),
        "版".repeat(86),
    ] {
        let body = InstallAgentSkillRequest {
            id: "research".into(),
            version,
        };
        assert!(
            matches!(
                server
                    .client
                    .agents()
                    .install_skill("agt_one", &body, &key)
                    .await,
                Err(Error::Validation { .. })
            ),
            "invalid version: {:?}",
            body.version
        );
    }
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
    let _ = (&server.anonymous, &server.base_url);
}

#[tokio::test]
async fn customization_reads_escape_agent_ids_and_reject_invalid_response_types() {
    let server = WireServer::new().await;
    server.expect(response(
        "GET",
        "/api/v1/agents/agt%2Fone/models",
        None,
        200,
        json!({"models":[{"id":"model","label":"Model","available":false}]}),
        true,
    ));
    let models = server.client.agents().models("agt/one").await.unwrap();
    assert!(!models.models[0].available);
    server.expect(response("GET", "/api/v1/agents/agt%2Fone/skills", None, 200,
        json!({"items":[],"pending":false,"can_install":false,"unavailable_reason":"Unsupported runtime"}), true));
    let skills = server.client.agents().skills("agt/one").await.unwrap();
    assert_eq!(
        skills.unavailable_reason.as_deref(),
        Some("Unsupported runtime")
    );
    for body in [
        json!({"items":[],"pending":false}),
        json!({"items":[{"id":"research","version":"1.0","source":"other","status":"installed"}],"pending":false,"can_install":true}),
        json!({"items":[{"id":"research","version":"1.0","source":"ornn","status":"unknown"}],"pending":false,"can_install":true}),
    ] {
        server.expect(response(
            "GET",
            "/api/v1/agents/agt_one/skills",
            None,
            200,
            body,
            true,
        ));
        assert!(matches!(
            server.client.agents().skills("agt_one").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[test]
fn skill_install_request_cannot_represent_unknown_or_missing_fields() {
    for body in [
        json!({"id":"research","version":"1.0","source":"ornn"}),
        json!({"id":"research"}),
        json!({"version":"1.0"}),
    ] {
        assert!(serde_json::from_value::<InstallAgentSkillRequest>(body).is_err());
    }
}
