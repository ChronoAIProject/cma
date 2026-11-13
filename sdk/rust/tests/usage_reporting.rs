#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    admin::{PatchSettingsRequest, Section, UsageReportingSettings},
    auth::AuthSession,
};
use serde_json::json;
use wire::WireServer;

#[tokio::test]
async fn bearer_command_readiness_is_optional_and_strictly_boolean() {
    let server = WireServer::new().await;
    for ready in [None, Some(false), Some(true)] {
        let mut body =
            json!({"authenticated":true,"subject":"owner","visitor":false,"capabilities":[]});
        if let Some(ready) = ready {
            body["api_commands"] = json!(ready);
        }
        server.expect(wire::json("GET", "/auth/session", None, 200, body, true));
        let AuthSession::SignedIn(identity) = server.client.auth().read_session().await.unwrap()
        else {
            panic!("expected signed-in session")
        };
        assert_eq!(identity.api_commands, ready);
    }
    server.expect(wire::json("GET", "/auth/session", None, 200, json!({"authenticated":true,"subject":"owner","visitor":false,"capabilities":[],"api_commands":"true"}), true));
    assert!(matches!(
        server.client.auth().read_session().await,
        Err(Error::Protocol { .. })
    ));
}

#[tokio::test]
async fn session_visibility_supports_old_servers_and_strict_boolean_values() {
    let server = WireServer::new().await;
    for visible in [None, Some(false), Some(true)] {
        let mut body =
            json!({"authenticated":true,"subject":"owner","visitor":false,"capabilities":[]});
        if let Some(visible) = visible {
            body["usage_reporting"] = json!({"measured_visible": visible});
        }
        server.expect(wire::json("GET", "/auth/session", None, 200, body, true));
        let AuthSession::SignedIn(identity) = server.client.auth().read_session().await.unwrap()
        else {
            panic!("expected signed-in session")
        };
        assert_eq!(
            identity.usage_reporting.map(|value| value.measured_visible),
            visible
        );
    }
    server.expect(wire::json("GET", "/auth/session", None, 200,
        json!({"authenticated":true,"subject":"owner","visitor":false,"capabilities":[],"usage_reporting":{"measured_visible":"true"}}), true));
    assert!(matches!(
        server.client.auth().read_session().await,
        Err(Error::Protocol { .. })
    ));
    server.expect(wire::json(
        "GET",
        "/auth/session",
        None,
        503,
        json!({"status":503,"code":"settings_unavailable","detail":"Settings unavailable."}),
        true,
    ));
    assert!(
        matches!(server.client.auth().read_session().await, Err(Error::Api { status: 503, code: Some(code), .. }) if code == "settings_unavailable")
    );
    let _ = (&server.anonymous, &server.base_url);
    assert_eq!(server.request_count(), 5);
    server.assert_drained();
}

#[tokio::test]
async fn admin_can_patch_only_usage_visibility_and_round_trip_additive_fields() {
    let server = WireServer::new().await;
    let mut settings = json!({
        "defaults": {"sandbox_profile":{"id":"sbp_default","revision":1},"workspace_profile":{"id":"wkp_default","revision":1},"agent_profile":{"id":"agp_default","revision":1}},
        "inactivity":{"pause_after_seconds":900}, "retention":{"agent_log_days":90},
        "limits":{"max_sandboxes_per_user":10,"visitor_max_sandboxes":1,"visitor_max_launches_per_day":5},
        "triggers":{"enabled":false,"orchestration_paused":false},"marketplace":{"enabled":true}
    });
    for visible in [false, true] {
        settings["usage_reporting"] = json!({"measured_visible":visible,"future":"preserved"});
        let request = PatchSettingsRequest {
            expected_revision: 1,
            usage_reporting: Some(Section::new(UsageReportingSettings {
                measured_visible: visible,
            })),
            ..Default::default()
        };
        server.expect(wire::json(
            "PATCH",
            "/api/v1/admin/settings",
            Some(json!({"expected_revision":1,"usage_reporting":{"measured_visible":visible}})),
            200,
            json!({"revision":2,"settings":settings}),
            true,
        ));
        let saved = server
            .client
            .admin()
            .patch_settings(&request)
            .await
            .unwrap();
        let section = saved.settings.usage_reporting.as_ref().unwrap();
        assert_eq!(section.values.measured_visible, visible);
        assert_eq!(section.extensions["future"], "preserved");
        let encoded = serde_json::to_value(saved.settings).unwrap();
        assert_eq!(encoded["usage_reporting"], settings["usage_reporting"]);
    }
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}
