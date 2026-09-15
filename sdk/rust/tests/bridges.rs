#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{Error, IdempotencyKey, PageOptions, bridges::*};
use serde_json::{Value, json};
use wire::WireServer;

fn config() -> Value {
    json!({"display_name":"Fixture Bridge","binding":{"bot_id":"bot","platform":"telegram","route_ids":["route"],"key_id":"key"},
        "partition":"fixed_shared_agent","placement":{"kind":"existing_agent","target":{"sandbox_id":"sbx-1","workspace_id":"wks-1","worktree_id":"wkt-1","agent_id":"agt-1"}},
        "admission":{"kind":"scoped","policy":{"senders":{"kind":"open"},"chats":{"kind":"open"},"groups":"mention_or_reply_to_bot"}},
        "reply":"final_response","limits":{"max_partitions":10,"max_sandboxes":10,"max_queued_messages":20,"max_messages_per_minute":60,"max_payload_bytes":4096,"inbox_retention_seconds":3600,"reply_secret_retention_seconds":600}})
}
fn bridge() -> Value {
    json!({"id":"brg-1","owner":{"kind":"person","subject":"owner"},"display_name":"Fixture Bridge","revision":1,"config_revision":1,
        "state":{"kind":"disabled","accepted_work":"suspend_original"},"emergency":{"ingress_stopped":false,"execution_stopped":false,"outbound_stopped":false},
        "bot_id":"bot","platform":"telegram","can_manage":true,"created_at":"2026-09-15T06:00:00Z","updated_at":"2026-09-15T06:00:00Z"})
}
fn detail() -> Value {
    json!({"bridge":bridge(),"config":config(),"callback_url":"https://cma.example/callback","credential":{"revision":0,"present":false,"revoked":false}})
}
fn receipt() -> Value {
    json!({"bridge_id":"brg-1","owner":{"kind":"person","subject":"owner"},"actor":{"kind":"human","subject":"owner"},
        "revision":2,"config_revision":1,"command":"disable","applied_at":"2026-09-15T06:00:00Z"})
}
fn command() -> ControlBridgeRequest {
    ControlBridgeRequest {
        expected_revision: 1,
        command: BridgeControl::Disable {
            accepted_work: BridgeAcceptedWork::DrainOriginal,
        },
    }
}
fn key() -> IdempotencyKey {
    IdempotencyKey::new("bridge-command").unwrap()
}
fn expect(
    server: &WireServer,
    method: &str,
    path: &str,
    body: Option<Value>,
    response: Value,
    keyed: bool,
) {
    let mut expected = wire::json(method, path, body, 200, response, true);
    if keyed {
        expected
            .headers
            .insert("idempotency-key".into(), key().as_str().into());
    }
    server.expect(expected);
}

#[tokio::test]
async fn historical_configuration_remains_readable_but_new_writes_require_explicit_shared_context()
{
    let server = WireServer::new().await;
    let _ = (&server.base_url, &server.anonymous);
    let mut historical = detail();
    historical["config"]["partition"] = json!("conversation");
    expect(
        &server,
        "GET",
        "/api/v1/bridges/brg-1/configuration",
        None,
        historical,
        false,
    );
    let old = server
        .client
        .bridges()
        .configuration("brg-1")
        .await
        .unwrap();
    assert_eq!(old.config.partition, BridgePartitionPolicy::Conversation);
    assert!(matches!(
        server
            .client
            .bridges()
            .update(
                "brg-1",
                &UpdateBridgeRequest {
                    expected_revision: 1,
                    config: old.config,
                    previous_work: Some(BridgeAcceptedWork::DrainOriginal),
                },
                &key()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn current_configuration_refuses_target_scope_callback_and_custody_inconsistency() {
    let server = WireServer::new().await;
    for (pointer, value) in [
        ("/bridge/id", json!("brg-another")),
        ("/bridge/can_manage", json!(false)),
        ("/bridge/config_revision", json!(2)),
        ("/config/binding/bot_id", json!("different")),
        (
            "/callback_url",
            json!("https://cma.example/callback?key=hidden"),
        ),
        ("/credential/present", json!(true)),
    ] {
        let mut response = detail();
        *response.pointer_mut(pointer).unwrap() = value;
        expect(
            &server,
            "GET",
            "/api/v1/bridges/brg-1/configuration",
            None,
            response,
            false,
        );
        assert!(matches!(
            server.client.bridges().configuration("brg-1").await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn receipts_reject_wrong_command_revision_identity_and_channel_attribution() {
    let server = WireServer::new().await;
    let command = command();
    for (field, value) in [
        ("bridge_id", json!("brg-other")),
        ("revision", json!(3)),
        ("config_revision", json!(3)),
        ("command", json!("enable")),
        (
            "actor",
            json!({"kind":"channel","bridge_id":"brg-1","grant_id":"grant","platform":"telegram","chat_id":"chat","sender_id":"sender"}),
        ),
        (
            "actor",
            json!({"kind":"api","subject":"owner","delegation_id":"delegated"}),
        ),
    ] {
        let mut response = receipt();
        response[field] = value;
        expect(
            &server,
            "POST",
            "/api/v1/bridges/brg-1/commands",
            Some(serde_json::to_value(&command).unwrap()),
            response,
            true,
        );
        assert!(matches!(
            server
                .client
                .bridges()
                .control("brg-1", &command, &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn inventory_enforces_requested_scope_bounds_order_and_continuation() {
    let server = WireServer::new().await;
    let options = BridgeListOptions {
        organization: Some("org".into()),
        page: PageOptions {
            cursor: Some("brg-0".into()),
            limit: Some(1),
        },
    };
    let mut row = bridge();
    row["owner"] = json!({"kind":"organization","subject":"org"});
    let mut stale = row.clone();
    stale["id"] = json!("brg-0");
    for response in [
        json!({"items":[bridge()]}),
        json!({"items":[row.clone(),row.clone()]}),
        json!({"items":[stale]}),
        json!({"items":[],"next_cursor":"brg-1"}),
        json!({"items":[row.clone()],"next_cursor":"brg-2"}),
    ] {
        expect(
            &server,
            "GET",
            "/api/v1/bridges?organization=org&cursor=brg-0&limit=1",
            None,
            response,
            false,
        );
        assert!(matches!(
            server.client.bridges().list(&options).await,
            Err(Error::Protocol { .. })
        ));
    }
    expect(
        &server,
        "GET",
        "/api/v1/bridges?organization=org&cursor=brg-0&limit=1",
        None,
        json!({"items":[row],"next_cursor":"brg-1"}),
        false,
    );
    assert_eq!(
        server
            .client
            .bridges()
            .list(&options)
            .await
            .unwrap()
            .next_cursor
            .as_deref(),
        Some("brg-1")
    );
    server.assert_drained();
}

#[tokio::test]
async fn selected_key_is_write_only_redacted_and_import_does_not_send_an_idempotency_header() {
    let server = WireServer::new().await;
    let body = ImportBridgeKeyRequest {
        expected_revision: 1,
        key: "fixture-selected-secret",
    };
    assert!(!format!("{body:?}").contains(body.key));
    expect(
        &server,
        "PUT",
        "/api/v1/bridges/brg-1/credential",
        Some(serde_json::to_value(&body).unwrap()),
        json!({"revision":1,"present":true,"revoked":false}),
        false,
    );
    let status = server
        .client
        .bridges()
        .import_key("brg-1", &body)
        .await
        .unwrap();
    assert!(status.present);
    expect(
        &server,
        "PUT",
        "/api/v1/bridges/brg-1/credential",
        Some(json!({"expected_revision":1,"key":body.key,"retain_control_plane_authority":true})),
        json!({"revision":1,"present":true,"revoked":false}),
        false,
    );
    assert!(
        server
            .client
            .bridges()
            .import_key_with_control_plane_authority("brg-1", &body)
            .await
            .unwrap()
            .present
    );
    assert!(!format!("{:?}", server.client.bridges()).contains(body.key));
    expect(
        &server,
        "PUT",
        "/api/v1/bridges/brg-1/credential",
        Some(serde_json::to_value(&body).unwrap()),
        json!({"revision":1,"present":true,"revoked":false,"key":"unexpected-secret-field"}),
        false,
    );
    assert!(matches!(
        server.client.bridges().import_key("brg-1", &body).await,
        Err(Error::Protocol { .. })
    ));
    expect(
        &server,
        "POST",
        "/api/v1/bridges/brg-1/credential/revoke",
        Some(json!({"expected_revision":1,"expected_custody_revision":1})),
        json!({"revision":3,"present":true,"revoked":true}),
        true,
    );
    assert!(matches!(
        server
            .client
            .bridges()
            .revoke_key("brg-1", 1, 1, &key())
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn invalid_fresh_inputs_and_revision_overflow_fail_before_http() {
    let server = WireServer::new().await;
    for (pointer, value) in [
        ("/binding/route_ids", json!(["route", "route"])),
        ("/limits/max_sandboxes", json!(11)),
        ("/limits/max_payload_bytes", json!(1_048_577)),
        ("/display_name", json!("x".repeat(201))),
        (
            "/admission/policy/senders",
            json!({"kind":"allowlist","ids":["sender","sender"]}),
        ),
    ] {
        let mut config = config();
        *config.pointer_mut(pointer).unwrap() = value;
        let body: CreateBridgeRequest = serde_json::from_value(
            json!({"owner":{"kind":"person","subject":"owner"},"config":config}),
        )
        .unwrap();
        assert!(matches!(
            server.client.bridges().create(&body, &key()).await,
            Err(Error::Validation { .. })
        ));
    }
    for value in [0, u64::MAX, i64::MAX as u64 + 1] {
        assert!(matches!(
            server.client.bridges().validate("brg-1", value).await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .bridges()
            .control(
                "brg-1",
                &ControlBridgeRequest {
                    expected_revision: i64::MAX as u64,
                    command: BridgeControl::Enable,
                },
                &key()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server
            .client
            .bridges()
            .control(
                "brg-1",
                &command(),
                &IdempotencyKey::new("k".repeat(201)).unwrap()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    for limit in [0, 101] {
        assert!(matches!(
            server
                .client
                .bridges()
                .list(&BridgeListOptions {
                    page: PageOptions {
                        limit: Some(limit),
                        cursor: None
                    },
                    ..Default::default()
                })
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn shared_placements_preserve_explicit_parent_and_reject_missing_trust_or_invalid_capacity() {
    for shared_workspace in [false, true] {
        let server = WireServer::new().await;
        let mut value = config();
        value["partition"] = "conversation_sender".into();
        value["placement"] = json!({"kind":"workspace_per_partition","sandbox_id":"parent-sandbox",
            "workspace_profile":{"id":"workspace-profile","revision":2},"agent_profile":{"id":"agent-profile","revision":3},
            "shared_trust":"all_participants_share_filesystem_processes_and_credentials",
            "capacity":{"keep_other_workspaces":1,"keep_other_agents_per_workspace":2}});
        if shared_workspace {
            value["placement"]["kind"] = "worktree_agent_per_partition".into();
            value["placement"]["workspace_id"] = "parent-workspace".into();
            value["placement"]
                .as_object_mut()
                .unwrap()
                .remove("workspace_profile");
        }
        let configured: BridgeConfiguration = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&configured).unwrap(), value);
        let mut invalid = value.clone();
        invalid["placement"]
            .as_object_mut()
            .unwrap()
            .remove("shared_trust");
        assert!(serde_json::from_value::<BridgeConfiguration>(invalid).is_err());
        for slots in [0, 16] {
            let mut invalid = value.clone();
            invalid["placement"]["capacity"]["keep_other_agents_per_workspace"] = slots.into();
            let request = CreateBridgeRequest {
                owner: cma_sdk::resource_access::ResourceOwner::Person {
                    subject: "owner".into(),
                },
                config: serde_json::from_value(invalid).unwrap(),
            };
            assert!(matches!(
                server.client.bridges().create(&request, &key()).await,
                Err(Error::Validation { .. })
            ));
        }
    }
}

#[tokio::test]
async fn capabilities_preserve_owner_scope_and_reject_unknown_contracts() {
    let server = WireServer::new().await;
    let empty = json!({"version":1,"source_revision":null,"connection_validation_available":false,"connectors":[]});
    expect(
        &server,
        "GET",
        "/api/v1/bridge-connectors/capabilities?organization=org-one",
        None,
        empty.clone(),
        false,
    );
    let catalog = server
        .client
        .bridges()
        .capabilities(Some("org-one"))
        .await
        .unwrap();
    assert!(!catalog.connection_validation_available);
    assert!(catalog.connectors.is_empty());
    let mut unknown = empty;
    unknown["version"] = json!(2);
    expect(
        &server,
        "GET",
        "/api/v1/bridge-connectors/capabilities",
        None,
        unknown,
        false,
    );
    assert!(matches!(
        server.client.bridges().capabilities(None).await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn complete_capability_projection_keeps_support_and_deployment_switches_distinct() {
    let server = WireServer::new().await;
    let connector = json!({"platform":"telegram","evidence":"contract_tested","live_verified":false,
        "inbound_text":true,"stable_sender_chat":true,"stable_thread":"telegram_forum_topic",
        "reply_to_bot":true,"bot_mentions":false,"inbound_edits":false,"inbound_deletes":false,
        "verified_actions":false,"attachment_input":"download_required","attachment_output":false,
        "output_text":true,"output_edit":false,"output_progress":true,"output_limit_utf16":4000,"output_format":"escaped_markdown",
        "stages":{"management":true,"ingress":false,"allocation":false,"execution":false,"outbound":false}});
    let mut catalog = json!({"version":1,"source_revision":"85607255ee0beadca460a350e0aa6d177edd2fd2",
        "connection_validation_available":true,"connectors":[connector]});
    expect(
        &server,
        "GET",
        "/api/v1/bridge-connectors/capabilities",
        None,
        catalog.clone(),
        false,
    );
    let parsed = server.client.bridges().capabilities(None).await.unwrap();
    assert_eq!(
        parsed.connectors[0].stable_thread,
        Some(BridgeStableThread::TelegramForumTopic)
    );
    assert!(parsed.connectors[0].output_text);
    assert!(!parsed.connectors[0].stages.outbound);
    catalog["connectors"][0]["output_text"] = json!(false);
    catalog["connectors"][0]["output_progress"] = json!(false);
    catalog["connectors"][0]["output_format"] = json!("unavailable");
    expect(
        &server,
        "GET",
        "/api/v1/bridge-connectors/capabilities",
        None,
        catalog,
        false,
    );
    assert!(matches!(
        server.client.bridges().capabilities(None).await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn telegram_new_choices_keep_the_distinct_public_platform_identity() {
    let server = WireServer::new().await;
    expect(
        &server,
        "GET",
        "/api/v1/bridge-connectors",
        None,
        json!({"bots":[{"id":"bot","platform":"telegram-new","active":true}],"keys":[],"routes":[]}),
        false,
    );
    let choices = server.client.bridges().choices(None).await.unwrap();
    assert_eq!(choices.bots[0].platform, BridgePlatform::TelegramNew);
    server.assert_drained();
}
