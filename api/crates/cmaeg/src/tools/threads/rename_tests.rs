use super::*;
use axum::http::StatusCode;
use cmaeg_core::channel::{Secret, Source};
use fixture::{thread, App};
use serde_json::{json, Value};
use std::time::Duration;

async fn request(app: &App, value: Value, key: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/thread-operations", app.base))
        .bearer_auth(&app.key)
        .header("Idempotency-Key", key)
        .json(&value)
        .send()
        .await
        .unwrap()
}
pub(super) async fn rename(app: &App, target: Value, name: &str, key: &str) -> Value {
    let response = request(
        app,
        json!({"target":target,"action":"rename","name":name}),
        key,
    )
    .await;
    let status = response.status();
    let value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}

#[test]
fn legacy_manage_operation_bytes_and_durable_hashes_are_unchanged() {
    for (action, wire, digest) in [
        (
            ThreadAction::Archive,
            "archive",
            "e6bfcb0a5f5ee99abf788cf045371e62b6dfdd723b0ca9e103a8af88a1cd0dd1",
        ),
        (
            ThreadAction::Reopen,
            "reopen",
            "694157a37ee7e6072a7d8e651f4fb6d8b4203f97c87674dc7284aa89cb21c762",
        ),
        (
            ThreadAction::Join,
            "join",
            "cb62d3df73592a9f115f780f580513d386bc4ac5df734a87ac2b6af8249a49be",
        ),
        (
            ThreadAction::Leave,
            "leave",
            "3ba62b2b7c38489ecc8d45332d55b68e62e33318baa47d0eceab06cdc367cf83",
        ),
    ] {
        let operation = Operation::Manage { action, name: None };
        assert_eq!(
            serde_json::to_string(&operation).unwrap(),
            format!(r#"{{"type":"manage","action":"{wire}"}}"#)
        );
        assert_eq!(hash(&operation), digest);
    }
}

#[tokio::test]
async fn rename_changes_only_the_exact_name_and_preserves_lock_custody_and_replay() {
    for kind in [10, 11, 12] {
        for named in [false, true] {
            let app = App::new(Some(kind), named).await;
            app.memory.lock().unwrap().channels.get_mut("99").unwrap()["thread_metadata"]
                ["locked"] = true.into();
            let result = rename(&app, app.target(), "Incident triage", "rename").await;
            assert_eq!(result["outcome"]["status"], "confirmed");
            assert_eq!(result["thread"]["name"], "Incident triage");
            assert_eq!(result["thread"]["locked"], true);
            assert_eq!(result["thread"]["archived"], false);
            let reference: ThreadRef =
                serde_json::from_value(result["thread"]["thread_ref"].clone()).unwrap();
            let payload = reference.open(&app.state.keys, now()).unwrap().payload;
            assert_eq!(payload.event, app.event);
            assert_eq!(payload.expires_at, app.event.expires_at);
            assert_eq!(
                rename(&app, app.target(), "Incident triage", "rename").await,
                result
            );
            assert_eq!(
                request(
                    &app,
                    json!({"target":app.target(),"action":"rename","name":"Different"}),
                    "rename"
                )
                .await
                .status(),
                StatusCode::CONFLICT
            );
            assert_eq!(
                app.writes(),
                vec![(
                    "PATCH".into(),
                    "/api/v10/channels/99".into(),
                    json!({"name":"Incident triage"})
                )]
            );
        }
    }
}

#[tokio::test]
async fn rename_accepts_scoped_list_and_create_refs_without_accepting_raw_destinations() {
    let app = App::new(None, true).await;
    let listing: Value = app.list("active").await.json().await.unwrap();
    app.memory
        .lock()
        .unwrap()
        .channels
        .insert("701".into(), thread("701", 11, "98", false));
    let listed = json!({"type":"thread","thread_ref":listing["threads"][0]["thread_ref"]});
    assert_eq!(
        rename(&app, listed, "Listed", "listed").await["outcome"]["status"],
        "confirmed"
    );
    let created: Value = app.create("create").await.json().await.unwrap();
    let created = json!({"type":"thread","thread_ref":created["thread"]["thread_ref"]});
    assert_eq!(
        rename(&app, created, "Created", "created").await["outcome"]["status"],
        "confirmed"
    );
    let writes = app.writes().len();
    assert_eq!(
        request(
            &app,
            json!({"target":{"type":"thread","thread_id":"701"},"action":"rename","name":"Raw"}),
            "raw"
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        rename(&app, app.target(), "Parent", "parent").await["outcome"]["status"],
        "unsupported"
    );
    assert_eq!(app.writes().len(), writes);
}

#[tokio::test]
async fn rename_never_reopens_archived_threads_and_refusals_release_the_fence() {
    let app = App::new(Some(11), false).await;
    app.memory.lock().unwrap().channels.get_mut("99").unwrap()["thread_metadata"]["archived"] =
        true.into();
    let result = rename(&app, app.target(), "Renamed", "archived").await;
    assert_eq!(
        result["outcome"],
        json!({"status":"unsupported","reason":"rename_requires_active_thread"})
    );
    assert!(app.writes().is_empty());
    let reopened: Value = app
        .manage(app.target(), "reopen", "explicit-reopen")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(reopened["outcome"]["status"], "confirmed");
    assert_eq!(
        rename(&app, app.target(), "Renamed", "active").await["outcome"]["status"],
        "confirmed"
    );
    assert_eq!(app.writes()[0].2, json!({"archived":false}));
    assert_eq!(app.writes()[1].2, json!({"name":"Renamed"}));
}

#[tokio::test]
async fn invalid_rename_and_name_on_legacy_actions_refuse_before_provider_reads() {
    let app = App::new(Some(11), false).await;
    for fields in [
        json!({"action":"rename"}),
        json!({"action":"rename","name":null}),
        json!({"action":"rename","name":""}),
        json!({"action":"rename","name":"bad\u{001b}"}),
        json!({"action":"rename","name":"🧵".repeat(101)}),
        json!({"action":"archive","name":"Name"}),
        json!({"action":"reopen","name":"Name"}),
        json!({"action":"join","name":"Name"}),
        json!({"action":"leave","name":"Name"}),
        json!({"action":"archive","name":null}),
    ] {
        let mut value = fields;
        value["target"] = app.target();
        assert_eq!(
            request(&app, value, "invalid").await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert!(app.memory.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn rename_refreshes_source_credentials_consumer_and_owner_before_dispatch() {
    for named in [false, true] {
        for case in 0..4 {
            let app = App::new(Some(11), named).await;
            app.change_on_read("/api/v10/channels/99", |definition| match case {
                0 => {
                    let Source::Discord {
                        thread_lifecycle, ..
                    } = &mut definition.sources[0]
                    else {
                        panic!()
                    };
                    *thread_lifecycle = false;
                }
                1 => definition.secrets.discord_bot_token = Some(Secret::new("rotated")),
                2 if named => {
                    definition.routing.as_mut().unwrap().consumers[0]
                        .provider
                        .binding_id = "rebound".parse().unwrap()
                }
                2 => crate::channels::definition::fixture::replace_default_binding(
                    definition,
                    "rebound".parse().unwrap(),
                ),
                _ => definition.owner.subject = "different-owner".into(),
            });
            assert_eq!(
                rename(&app, app.target(), "Revoked", "revoked").await["outcome"]["status"],
                "rejected"
            );
            assert!(app.writes().is_empty());
        }
    }
}

#[tokio::test]
async fn unconfirmed_rename_response_fences_every_later_action_without_retry() {
    for case in 0..6 {
        let app = App::new(Some(11), false).await;
        let mut response = thread("99", 11, "98", false);
        response["name"] = "Requested".into();
        match case {
            0 => response["id"] = "88".into(),
            1 => response["parent_id"] = "97".into(),
            2 => response["guild_id"] = "200".into(),
            3 => response["type"] = 12.into(),
            4 => response["name"] = "Different".into(),
            _ => response["thread_metadata"]["archived"] = true.into(),
        }
        app.memory.lock().unwrap().patch_response = Some(response);
        let result = rename(&app, app.target(), "Requested", "uncertain").await;
        assert_eq!(result["outcome"]["status"], "unknown", "case {case}");
        assert!(result.get("thread").is_none());
        app.memory.lock().unwrap().patch_response = None;
        assert_eq!(
            rename(&app, app.target(), "Requested", "uncertain").await,
            result
        );
        assert_eq!(
            rename(&app, app.target(), "Other", "another").await["outcome"]["status"],
            "unknown"
        );
        let archived: Value = app
            .manage(app.target(), "archive", "archive")
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(archived["outcome"]["status"], "unknown");
        assert_eq!(app.writes().len(), 1);
        app.backend.expire_at(app.event.expires_at + 1);
        let store = ledger::Store::new(app.state.resolver.record_client(), app.state.keys.clone());
        assert!(store
            .acquire("77", "99", &app.definition.owner.subject)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn rename_permission_and_rate_limit_refusals_are_definite_and_never_retried() {
    for (status, outcome) in [
        (StatusCode::FORBIDDEN, "rejected"),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
    ] {
        let app = App::new(Some(11), false).await;
        app.memory.lock().unwrap().status = status;
        let result = rename(&app, app.target(), "Refused", "refused").await;
        assert_eq!(result["outcome"]["status"], outcome);
        assert_eq!(
            rename(&app, app.target(), "Refused", "refused").await,
            result
        );
        assert_eq!(app.writes().len(), 1);
        app.memory.lock().unwrap().status = StatusCode::OK;
        let next = rename(&app, app.target(), "Allowed", "new-intent").await;
        if status == StatusCode::FORBIDDEN {
            assert_eq!(next["outcome"]["status"], "confirmed");
        } else {
            assert_eq!(next["outcome"]["status"], "rate_limited");
            assert_eq!(app.writes().len(), 1);
        }
    }
}

#[tokio::test]
async fn rename_defers_creator_or_moderator_permission_to_discord() {
    let app = App::new(Some(11), false).await;
    // A moderator bot can rename someone else's thread. Creator status is not a local grant.
    app.memory.lock().unwrap().channels.get_mut("99").unwrap()["owner_id"] = "51".into();
    assert_eq!(
        rename(&app, app.target(), "Moderated", "moderator").await["outcome"]["status"],
        "confirmed"
    );
    app.memory.lock().unwrap().status = StatusCode::FORBIDDEN;
    assert_eq!(
        rename(&app, app.target(), "Denied", "permission-revoked").await["outcome"]["status"],
        "rejected"
    );
    assert_eq!(app.writes().len(), 2);
}

#[tokio::test]
async fn concurrent_rename_and_archive_serialize_on_the_existing_exact_thread_fence() {
    let app = App::new(Some(11), false).await;
    app.memory.lock().unwrap().delay = Some((
        "PATCH".into(),
        "/api/v10/channels/99".into(),
        Duration::from_millis(100),
    ));
    let (renamed, archived) = tokio::join!(
        rename(&app, app.target(), "Concurrent", "rename"),
        app.manage(app.target(), "archive", "archive")
    );
    let archived: Value = archived.json().await.unwrap();
    let states = [
        renamed["outcome"]["status"].as_str().unwrap(),
        archived["outcome"]["status"].as_str().unwrap(),
    ];
    assert_eq!(states.iter().filter(|s| **s == "confirmed").count(), 1);
    assert_eq!(states.iter().filter(|s| **s == "unknown").count(), 1);
    assert_eq!(app.writes().len(), 1);
}

#[tokio::test]
async fn timed_out_rename_keeps_the_persistent_fence() {
    let mut app = App::new(Some(11), false).await;
    let mut client = DiscordThreads::new(app.state.discord_threads.as_ref().unwrap().api.clone());
    client.budget = Duration::from_millis(500);
    app.state.discord_threads = Some(Arc::new(client));
    app.mount().await;
    app.memory.lock().unwrap().delay = Some((
        "PATCH".into(),
        "/api/v10/channels/99".into(),
        Duration::from_secs(1),
    ));
    let result = rename(&app, app.target(), "Slow", "slow").await;
    assert_eq!(result["outcome"]["status"], "unknown");
    app.memory.lock().unwrap().delay = None;
    assert_eq!(rename(&app, app.target(), "Slow", "slow").await, result);
    assert_eq!(
        rename(&app, app.target(), "Other", "new-key").await["outcome"]["status"],
        "unknown"
    );
    assert_eq!(app.writes().len(), 1);
}

#[tokio::test]
async fn mcp_rename_and_rest_share_the_exact_operation_and_retained_receipt() {
    let app = App::new(Some(11), true).await;
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"manageThread","arguments":{"request":{"target":app.target(),"action":"rename","name":"MCP triage"},"idempotency_key":"mcp-rename"}}});
    let response = reqwest::Client::new()
        .post(format!("{}/mcp", app.base))
        .bearer_auth(&app.key)
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value = response.json().await.unwrap();
    assert!(value.to_string().contains("confirmed"), "{value}");
    let replay = rename(&app, app.target(), "MCP triage", "mcp-rename").await;
    assert_eq!(replay["outcome"]["status"], "confirmed");
    assert_eq!(replay["thread"]["name"], "MCP triage");
    assert_eq!(
        app.writes(),
        vec![(
            "PATCH".into(),
            "/api/v10/channels/99".into(),
            json!({"name":"MCP triage"})
        )]
    );
}
