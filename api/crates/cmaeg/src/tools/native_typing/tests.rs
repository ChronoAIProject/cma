use super::{fixture::Fixture, *};
use cmaeg_core::{channel::Secret, seal::EventRef};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn original_default_and_named_consumers_can_type_in_private_and_thread_channels() {
    for named in [false, true] {
        for whatsapp in [false, true] {
            let f = if whatsapp {
                Fixture::whatsapp(named, 1).await
            } else {
                Fixture::discord(named, true).await
            };
            assert!(f.capability().await);
            assert!(
                f.app
                    .state
                    .context(f.app.caller(), &f.reference())
                    .await
                    .unwrap()
                    .capabilities
                    .typing
            );
            assert_eq!(
                f.writes(),
                0,
                "capability discovery never sends platform writes"
            );
            assert_eq!(f.typing().await.unwrap().outcome, TypingOutcome::Sent);
            assert_eq!(f.writes(), 1);
            if whatsapp {
                assert_eq!(
                    f.graph.writes(),
                    vec![json!({"messaging_product":"whatsapp",
                    "status":"read","message_id":"wamid.original","typing_indicator":{"type":"text"}})]
                );
            } else {
                assert_eq!(
                    f.discord.memory.lock().unwrap().writes[0].1,
                    "/api/v10/channels/99/typing"
                );
            }
        }
    }
}

#[tokio::test]
async fn default_off_and_provider_gates_preserve_unsupported_without_external_calls() {
    for whatsapp in [false, true] {
        let mut f = if whatsapp {
            Fixture::whatsapp(false, 1).await
        } else {
            Fixture::discord(false, false).await
        };
        f.app.state.native_typing = None;
        assert!(!f.capability().await);
        assert_eq!(
            f.typing().await.unwrap().outcome,
            TypingOutcome::Unsupported
        );
        f.app.state.native_typing = Some(Arc::new(NativeTyping::new(
            f.graph.client(),
            f.discord.api.clone(),
            false,
            false,
        )));
        assert!(!f.capability().await);
        assert_eq!(
            f.typing().await.unwrap().outcome,
            TypingOutcome::Unsupported
        );
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn whatsapp_requires_recent_original_private_message_and_exact_native_bytes() {
    for age in [86_400, 172_800, -600] {
        let f = Fixture::whatsapp(false, age).await;
        assert!(!f.capability().await);
        assert!(f.typing().await.is_err());
        assert!(f.graph.calls.lock().unwrap().is_empty());
    }
    for change in 0..6 {
        let f = Fixture::with_wire(false, Some(1), false, |wire| {
            let message = &mut wire["entry"][0]["changes"][0]["value"]["messages"][0];
            match change {
                0 => message["id"] = "wamid.other".into(),
                1 => message["from"] = "447700900999".into(),
                2 => message["timestamp"] = "1".into(),
                3 => {
                    message["type"] = "reaction".into();
                    message["reaction"] = json!({"message_id":"wamid.other","emoji":"👍"});
                }
                4 => {
                    wire["entry"][0]["changes"][0]["value"]["metadata"]["phone_number_id"] =
                        "99999".into()
                }
                5 => message["group_id"] = "120363123456789@g.us".into(),
                _ => unreachable!(),
            }
        })
        .await;
        assert!(!f.capability().await, "change {change}");
        assert!(f.typing().await.is_err());
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn discord_rejects_changed_original_ids_bot_system_webhook_and_nonmessage_types() {
    for change in 0..7 {
        let f = Fixture::with_wire(false, None, true, |wire| match change {
            0 => wire["dispatch"]["id"] = "801".into(),
            1 => wire["dispatch"]["channel_id"] = "98".into(),
            2 => wire["dispatch"]["author"]["id"] = "41".into(),
            3 => wire["dispatch"]["author"]["bot"] = true.into(),
            4 => wire["dispatch"]["author"]["system"] = true.into(),
            5 => wire["dispatch"]["webhook_id"] = "50".into(),
            6 => wire["dispatch"]["type"] = 6.into(),
            _ => unreachable!(),
        })
        .await;
        assert!(!f.capability().await, "change {change}");
        assert!(f.typing().await.is_err());
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn changed_event_identity_expiry_or_other_consumer_never_authorizes_typing() {
    for change in 0..5 {
        let mut f = Fixture::discord(true, true).await;
        match change {
            0 => f.app.event.activity_sha256 = None,
            1 => f.app.event.expires_at = 1,
            2 => f.app.event.consumer = None,
            3 => f.app.event.event_id = "discord:77:801".parse().unwrap(),
            4 => {
                if let ReplyAddress::Discord { channel_id, .. } = &mut f.app.event.reply {
                    *channel_id = "98".into();
                }
            }
            _ => unreachable!(),
        }
        assert!(f.typing().await.is_err(), "change {change}");
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn owner_source_provider_and_key_revocation_are_rechecked_after_platform_reads() {
    for whatsapp in [false, true] {
        for change in [0, 1, 2, if whatsapp { 3 } else { 4 }, 5, 6] {
            let mut f = if whatsapp {
                Fixture::whatsapp(false, 1).await
            } else {
                Fixture::discord(false, true).await
            };
            let entered = Arc::new(Semaphore::new(0));
            let release = Arc::new(Semaphore::new(0));
            if whatsapp {
                *f.graph.gate.lock().unwrap() = Some((entered.clone(), release.clone()));
            } else {
                f.discord.memory.lock().unwrap().message_gate =
                    Some((entered.clone(), release.clone()));
            }
            let state = f.app.state.clone();
            let caller = f.app.caller();
            let reference = f.reference();
            let pending = tokio::spawn(async move { state.typing(caller, &reference).await });
            tokio::time::timeout(Duration::from_secs(3), entered.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
            f.rotate(change);
            release.add_permits(1);
            assert!(
                pending.await.unwrap().is_err(),
                "platform {whatsapp}, change {change}"
            );
            assert_eq!(f.writes(), 0);
        }
    }
}

#[tokio::test]
async fn named_consumer_credential_and_removal_are_checked_again_before_write() {
    for removed in [false, true] {
        let mut f = Fixture::whatsapp(true, 1).await;
        let entered = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        *f.graph.gate.lock().unwrap() = Some((entered.clone(), release.clone()));
        let state = f.app.state.clone();
        let caller = f.app.caller();
        let reference = f.reference();
        let pending = tokio::spawn(async move { state.typing(caller, &reference).await });
        tokio::time::timeout(Duration::from_secs(3), entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        if removed {
            f.app.definition.routing = None;
        } else {
            f.app
                .definition
                .secrets
                .consumer_keys
                .iter_mut()
                .find(|(id, _)| Some(id) == f.app.event.consumer.as_ref())
                .unwrap()
                .1 = Secret::new("replaced-named-consumer");
        }
        f.app.replace();
        release.add_permits(1);
        assert!(pending.await.unwrap().is_err());
        assert_eq!(f.writes(), 0);
    }
}

#[tokio::test]
async fn rest_and_mcp_keep_existing_typing_response_shape_and_authentication() {
    let f = Fixture::whatsapp(false, 1).await;
    let routes = crate::http::tools::router(f.app.state.clone())
        .merge(crate::mcp::router(f.app.state.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let reference = f.reference();
    let path = format!("{base}/v1/events/{}/typing", reference.as_str());
    let response = client
        .post(&path)
        .bearer_auth(&f.app.key)
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"outcome":"sent"})
    );
    let response = client
        .post(format!("{base}/mcp"))
        .bearer_auth(&f.app.key)
        .json(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"sendTyping","arguments":{"event_ref":reference}}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["result"]["structuredContent"],
        json!({"outcome":"sent"})
    );
    assert_eq!(
        client.post(&path).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.writes(), 2);
    server.abort();
}

#[tokio::test]
async fn relay_lark_stays_unsupported_and_typing_never_marks_a_reply_delivered() {
    let f = Fixture::whatsapp(false, 1).await;
    f.typing().await.unwrap();
    // Typing must not suppress the provider's eventual actual response.
    assert!(!f
        .app
        .state
        .tracker
        .has_tool_reply(f.reference().as_str(), super::now()));
    let mut app =
        crate::tools::message_effects::lark_fixture::App::new(cmaeg_core::channel::Platform::Lark)
            .await;
    if let ReplyAddress::NyxidRelay {
        reply_token_expires_at,
        ..
    } = &mut app.event.reply
    {
        *reply_token_expires_at = app.event.expires_at;
    }
    app.state.native_typing = f.app.state.native_typing.clone();
    let reference = EventRef::seal(&app.event, &app.state.keys).unwrap();
    assert_eq!(
        app.state
            .typing(app.caller(), &reference)
            .await
            .unwrap()
            .outcome,
        TypingOutcome::Unsupported
    );
    assert!(app.writes().is_empty());
}

#[tokio::test]
async fn next_whatsapp_request_uses_rotated_current_credentials_without_old_token_cache() {
    let mut f = Fixture::whatsapp(false, 1).await;
    f.typing().await.unwrap();
    f.rotate(3);
    *f.graph.expected_access.lock().unwrap() = "rotated-wa-fixture".into();
    assert!(f.capability().await);
    assert_eq!(f.typing().await.unwrap().outcome, TypingOutcome::Sent);
    assert_eq!(f.writes(), 2);
}
