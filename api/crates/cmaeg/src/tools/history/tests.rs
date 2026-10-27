use super::*;
use axum::http::StatusCode;
use cmaeg_core::channel::{Secret, Source};
use serde_json::{json, Value};
#[path = "fixture.rs"]
mod fixture;
use fixture::App;

async fn body(response: reqwest::Response) -> Value {
    let status = response.status();
    let value = response.json().await.unwrap();
    assert_eq!(status, 200, "{value}");
    value
}
#[tokio::test]
async fn eligible_origins_and_named_consumers_have_exact_read_only_scope() {
    for kind in [0, 1, 2, 5, 10, 11, 12, 13] {
        for named in [false, true] {
            let app = App::new(kind, named).await;
            app.backend
                .fail_put
                .store(true, std::sync::atomic::Ordering::SeqCst);
            app.set_rows(1);
            let page = body(app.read(None).await).await;
            assert_eq!(page["conversation"]["id"], app.activity.conversation.id);
            assert_eq!(page["messages"][0]["message_id"], "699");
            assert_eq!(page["expires_at"], app.event.expires_at);
            assert_eq!(page["content_coverage"], "platform_returned_only");
            let memory = app.memory.lock().unwrap();
            assert_eq!(
                memory
                    .calls
                    .iter()
                    .filter(|(_, p, _)| p.contains("?before="))
                    .count(),
                1
            );
            assert_eq!(
                memory
                    .calls
                    .iter()
                    .filter(|(_, p, _)| p.ends_with("/messages/700"))
                    .count(),
                2
            );
            assert!(memory.calls.iter().all(|(m, p, _)| m == "GET"
                && !p.contains("/guilds/")
                && !p.contains("/threads/")));
        }
    }
}
#[tokio::test]
async fn pages_are_exclusive_live_and_keep_original_expiry_after_boundary_deletion() {
    let app = App::new(12, true).await;
    app.set_rows(55);
    let first = body(app.read(None).await).await;
    assert_eq!(first["scanned_messages"], 25);
    assert_eq!(first["messages"][0]["message_id"], "675");
    let cursor = first["continuation"]["cursor"].as_str().unwrap();
    {
        let mut memory = app.memory.lock().unwrap();
        memory
            .rows
            .as_array_mut()
            .unwrap()
            .retain(|r| r["id"] != "675");
    }
    let second = body(app.read(Some(cursor)).await).await;
    assert_eq!(second["before_message_id"], "675");
    assert_eq!(second["messages"][24]["message_id"], "674");
    assert_eq!(second["expires_at"], first["expires_at"]);
    {
        let mut memory = app.memory.lock().unwrap();
        memory
            .rows
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["id"] == "674")
            .unwrap()["content"] = "Edited later".into();
    }
    let replay = body(app.read(Some(cursor)).await).await;
    assert_eq!(replay["messages"][24]["text"], "Edited later");
    let third = body(app.read(second["continuation"]["cursor"].as_str()).await).await;
    assert_eq!(third["scanned_messages"], 5);
    assert_eq!(third["continuation"]["status"], "end_of_visible_history");
    assert!(third["continuation"].get("cursor").is_none());
    assert!(app.writes().is_empty());
}
#[tokio::test]
async fn unsupported_and_missing_admitted_evidence_never_read_the_platform() {
    for kind in [3, 4, 15, 16, 99] {
        let app = App::new(kind, false).await;
        assert!(!app.read(None).await.status().is_success());
        assert!(app.memory.lock().unwrap().calls.is_empty());
    }
    let app = App::with_setup(0, false, |a| {
        a.source_body
            .as_object_mut()
            .unwrap()
            .remove("native_channel");
    })
    .await;
    assert!(!app.read(None).await.status().is_success());
    assert!(app.memory.lock().unwrap().calls.is_empty());
}
#[tokio::test]
async fn consent_at_admission_and_current_source_are_both_required() {
    let mut app = App::with_setup(0, false, |a| {
        if let Source::Discord {
            conversation_history,
            ..
        } = &mut a.definition.sources[0]
        {
            *conversation_history = false;
        }
        a.source_body["channel"] = a.definition.to_public_json();
    })
    .await;
    if let Source::Discord {
        conversation_history,
        ..
    } = &mut app.definition.sources[0]
    {
        *conversation_history = true;
    }
    app.save();
    assert_eq!(app.read(None).await.status(), 403);
    assert!(app.memory.lock().unwrap().calls.is_empty());
    let app = App::new(0, false).await;
    let mut revoked = app.definition.clone();
    if let Source::Discord {
        conversation_history,
        thread_history,
        thread_lifecycle,
        ..
    } = &mut revoked.sources[0]
    {
        *conversation_history = false;
        *thread_history = true;
        *thread_lifecycle = true;
    }
    app.seed(&revoked);
    assert_eq!(app.read(None).await.status(), 403);
}
#[tokio::test]
async fn final_custody_and_anchor_checks_discard_even_empty_pages() {
    for case in 0..7 {
        let app = App::new(0, false).await;
        app.change_on_read(
            "/api/v10/channels/98/messages",
            move |definition| match case {
                0 => {
                    if let Source::Discord {
                        conversation_history,
                        ..
                    } = &mut definition.sources[0]
                    {
                        *conversation_history = false;
                    }
                }
                1 => definition.secrets.discord_bot_token = Some(Secret::new("changed-bot-token")),
                2 => crate::channels::definition::fixture::replace_default_agent_key(
                    definition,
                    "changed-agent-key",
                ),
                3 => definition.owner.subject = "changed-owner".into(),
                4 => {
                    definition.secrets.provider_credential =
                        Some(Secret::new("changed-provider-token"))
                }
                5 => definition.provider.binding_id = "rebound".parse().unwrap(),
                _ => definition.partition.version += 1,
            },
        );
        assert!(!app.read(None).await.status().is_success(), "case {case}");
        assert!(app.writes().is_empty());
    }
    for case in 0..4 {
        let app = App::new(12, false).await;
        app.set_rows(1);
        {
            let mut memory = app.memory.lock().unwrap();
            if case == 3 {
                memory.final_channel =
                    Some(json!({"id":"99","type":12,"guild_id":"100","parent_id":"97"}));
            } else {
                let mut anchor = memory.message.clone();
                match case {
                    0 => anchor["id"] = "701".into(),
                    1 => anchor["author"]["id"] = "52".into(),
                    _ => anchor["author"]["bot"] = true.into(),
                };
                memory.final_anchor = Some(anchor);
            }
        }
        assert_eq!(app.read(None).await.status(), 403);
    }
}
#[tokio::test]
async fn request_inputs_and_cursor_custody_cannot_select_destinations_or_renew_expiry() {
    let app = App::new(0, false).await;
    app.set_rows(26);
    let page = body(app.read(None).await).await;
    let cursor = page["continuation"]["cursor"].as_str().unwrap();
    for query in [
        "limit=10",
        "channel_id=99",
        "before=500",
        "after=500",
        "around=500",
        "offset=1",
        "cursor=a&cursor=b",
        "cursor=",
    ] {
        let response = reqwest::Client::new()
            .get(format!(
                "{}/v1/events/{}/history?{query}",
                app.base,
                app.reference().as_str()
            ))
            .bearer_auth(&app.key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{query}");
    }
    let original = cursor
        .parse::<HistoryCursor>()
        .unwrap()
        .open(&app.state.keys, now())
        .unwrap()
        .payload;
    for case in 0..5 {
        let mut payload = original.clone();
        match case {
            0 => payload.event_sha256 = "c".repeat(64),
            1 => payload.custody = "d".repeat(64),
            2 => payload.origin.channel_id = "97".into(),
            3 => payload.anchor_message_id = "701".into(),
            _ => payload.expires_at += 1,
        };
        let forged = HistoryCursor::seal(&payload, &app.state.keys).unwrap();
        assert_eq!(app.read(Some(forged.as_str())).await.status(), 400);
    }
    let mut changed = app.definition.clone();
    changed.secrets.discord_bot_token = Some(Secret::new("new-token"));
    app.seed(&changed);
    assert_eq!(app.read(Some(cursor)).await.status(), 400);
}
#[tokio::test]
async fn mcp_exposes_only_enabled_read_with_strict_arguments() {
    let app = App::new(1, false).await;
    app.set_rows(1);
    let response=reqwest::Client::new().post(format!("{}/mcp",app.base)).bearer_auth(&app.key)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"readEventHistory","arguments":{"event_ref":app.reference()}}})).send().await.unwrap();
    let value: Value = response.json().await.unwrap();
    assert!(
        value.to_string().contains("platform_returned_only"),
        "{value}"
    );
    let response=reqwest::Client::new().post(format!("{}/mcp",app.base)).bearer_auth(&app.key)
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"readEventHistory","arguments":{"event_ref":app.reference(),"thread_id":"99"}}})).send().await.unwrap();
    let value: Value = response.json().await.unwrap();
    assert!(
        value.to_string().contains("invalid history arguments"),
        "{value}"
    );
}
#[tokio::test]
async fn busy_and_total_deadline_refuse_without_late_page_or_extra_platform_reads() {
    let app = App::with_setup(0, false, |a| {
        Arc::get_mut(a.state.discord_conversation_history.as_mut().unwrap())
            .unwrap()
            .budget = Duration::from_millis(60);
    })
    .await;
    {
        let client = app.state.history_client().unwrap();
        let _permits = client.slots.acquire_many(8).await.unwrap();
        assert_eq!(app.read(None).await.status(), 429);
        assert!(app.memory.lock().unwrap().calls.is_empty());
    }
    app.memory.lock().unwrap().delay = Some((
        "GET".into(),
        "/api/v10/channels/98/messages".into(),
        Duration::from_millis(150),
    ));
    let start = Instant::now();
    assert_eq!(app.read(None).await.status(), 503);
    assert!(start.elapsed() < Duration::from_millis(140));
    tokio::time::sleep(Duration::from_millis(170)).await;
    assert_eq!(
        app.memory
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, p, _)| p.ends_with("/messages/700"))
            .count(),
        1
    );
}
#[tokio::test]
async fn rate_limit_and_deleted_original_anchor_are_refusals_not_empty_successes() {
    let app = App::new(0, false).await;
    app.memory.lock().unwrap().status = StatusCode::TOO_MANY_REQUESTS;
    assert_eq!(app.read(None).await.status(), 429);
    let app = App::new(0, false).await;
    app.memory.lock().unwrap().message = Value::Null;
    assert_eq!(app.read(None).await.status(), 403);
    assert!(!app
        .memory
        .lock()
        .unwrap()
        .calls
        .iter()
        .any(|(_, p, _)| p.contains("?before=")));
}

#[tokio::test]
async fn deadline_includes_initial_storage_and_gate_disablement_has_no_remote_reads() {
    let app = App::with_setup(0, false, |a| {
        Arc::get_mut(a.state.discord_conversation_history.as_mut().unwrap())
            .unwrap()
            .budget = Duration::from_millis(80);
    })
    .await;
    app.backend
        .get_delay_ms
        .store(200, std::sync::atomic::Ordering::Relaxed);
    let started = Instant::now();
    assert_eq!(app.read(None).await.status(), 503);
    assert!(started.elapsed() < Duration::from_millis(180));
    assert!(app.memory.lock().unwrap().calls.is_empty());
    let app = App::with_setup(0, false, |a| {
        a.state.discord_conversation_history = None;
    })
    .await;
    assert_eq!(app.read(None).await.status(), 403);
    assert!(app.memory.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn oversized_provider_body_is_not_a_partial_success_and_empty_rows_can_continue() {
    let app = App::new(0, false).await;
    app.set_rows(26);
    {
        let mut memory = app.memory.lock().unwrap();
        for row in memory.rows.as_array_mut().unwrap() {
            row["type"] = 6.into();
        }
    }
    let value = body(app.read(None).await).await;
    assert_eq!(value["messages"], json!([]));
    assert_eq!(value["omitted_message_count"], 25);
    assert!(value["continuation"]["cursor"].is_string());
    app.memory.lock().unwrap().rows[0]["content"] = "x".repeat(1024 * 1024).into();
    assert_eq!(app.read(None).await.status(), 503);
}

#[tokio::test]
async fn edited_original_anchor_remains_eligible_but_unsupported_admitted_lineage_does_not() {
    let app = App::new(0, false).await;
    app.memory.lock().unwrap().message["content"] = "Edited original text".into();
    assert_eq!(app.read(None).await.status(), 200);
    for field in ["component", "message_event", "pending_reaction"] {
        let app = App::with_setup(0, false, |a| {
            a.source_body[field] = json!({"untrusted":"not native human evidence"});
        })
        .await;
        assert!(!app.read(None).await.status().is_success());
        assert!(app.memory.lock().unwrap().calls.is_empty());
    }
}

#[tokio::test]
async fn mcp_caps_the_final_envelope_including_its_mirrored_text() {
    let app = App::new(0, false).await;
    app.set_rows(25);
    {
        let mut memory = app.memory.lock().unwrap();
        for row in memory.rows.as_array_mut().unwrap() {
            row["content"] = "\"".repeat(8192).into();
            row["attachments"]=json!((0..10).map(|i|json!({"id":(100+i).to_string(),"filename":"\"".repeat(100),"size":100,"content_type":"\"".repeat(100),"description":"\"".repeat(100),"width":4294967295u32,"height":4294967295u32,"duration_secs":12345678.9,"ephemeral":true})).collect::<Vec<_>>());
        }
    }
    let page = body(app.read(None).await).await;
    assert!(serde_json::to_vec(&page).unwrap().len() < HISTORY_OUTPUT_BYTES);
    let mirrored = json!({"content":[{"type":"text","text":page.to_string()}],"structuredContent":page,"isError":false});
    assert!(serde_json::to_vec(&mirrored).unwrap().len() > HISTORY_OUTPUT_BYTES);
    let response=reqwest::Client::new().post(format!("{}/mcp",app.base)).bearer_auth(&app.key)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"readEventHistory","arguments":{"event_ref":app.reference()}}})).send().await.unwrap();
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["error"]["code"], -32000, "{value}");
    assert!(value.to_string().contains("history_service_unavailable"));
    assert!(!value.to_string().contains("more_visible_rows"));
}

#[tokio::test]
async fn discovery_needs_retained_consent_and_wrong_consumers_never_read_native_data() {
    let app = App::new(0, true).await;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    let caller = AgentCaller::from_headers(&headers).unwrap();
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let capability = app
        .state
        .history_capabilities(caller, &app.event, &channel)
        .await
        .unwrap();
    assert_eq!(capability.page_size, 25);
    assert_eq!(capability.expires_at, app.event.expires_at);
    assert!(app.memory.lock().unwrap().calls.is_empty());
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/events/{}/history",
            app.base,
            app.reference().as_str()
        ))
        .bearer_auth(fixture::DEFAULT_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert!(app.memory.lock().unwrap().calls.is_empty());
    let response = reqwest::Client::new()
        .get(format!("{}/v1/events/cmaeg1.tr.fixture/history", app.base))
        .bearer_auth(&app.key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let app = App::with_setup(0, false, |a| {
        if let Source::Discord {
            conversation_history,
            ..
        } = &mut a.definition.sources[0]
        {
            *conversation_history = false;
        }
        a.source_body["channel"] = a.definition.to_public_json();
    })
    .await;
    headers.insert(
        "authorization",
        format!("Bearer {}", app.key).parse().unwrap(),
    );
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    assert!(app
        .state
        .history_capabilities(
            AgentCaller::from_headers(&headers).unwrap(),
            &app.event,
            &channel
        )
        .await
        .is_none());
}

#[tokio::test]
async fn expiry_during_platform_read_discards_data_and_never_renews_the_event() {
    let app = App::with_setup(0, false, |a| {
        a.event.expires_at = now() + 2;
    })
    .await;
    app.set_rows(26);
    app.memory.lock().unwrap().delay = Some((
        "GET".into(),
        "/api/v10/channels/98/messages".into(),
        Duration::from_millis(2100),
    ));
    let response = app.read(None).await;
    assert_eq!(response.status(), 410);
    let value: Value = response.json().await.unwrap();
    assert!(!value.to_string().contains("cmaeg1.hc."));
    assert!(app.writes().is_empty());
}

#[path = "discovery_tests.rs"]
mod discovery;
