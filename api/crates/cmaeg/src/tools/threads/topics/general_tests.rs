//! General authority comes from a real forum envelope, while the public thread stays absent.
use super::*;

fn general_message() -> Value {
    json!({"update_id":101,"message":{"message_id":800,"date":now()-5,
        "chat":{"id":-100,"type":"supergroup","is_forum":true},
        "from":{"id":51,"is_bot":false,"first_name":"Alice"},
        "text":"Please manage this General topic"}})
}
async fn general(named: bool) -> (App, Arc<Telegram>) {
    fixture_with_native_message(named, true, general_message(), true).await
}
async fn rename(app: &App, name: &str, key: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/thread-operations", app.base))
        .bearer_auth(&app.key)
        .header("idempotency-key", key)
        .json(&json!({"target":app.target(),"action":"rename","name":name}))
        .send()
        .await
        .unwrap()
}
fn client(telegram: &Arc<Telegram>, enabled: bool) -> Arc<TelegramTopics> {
    Arc::new(
        TelegramTopics::new(Arc::new(
            TelegramClient::production(Duration::from_secs(1))
                .unwrap()
                .with_transport(telegram.clone()),
        ))
        .with_general_topics(enabled),
    )
}

#[tokio::test]
async fn general_rest_methods_preserve_absent_thread_and_original_consumer_with_exact_replay() {
    for named in [false, true] {
        let (app, telegram) = general(named).await;
        assert!(app.activity.conversation.thread_id.is_none());
        assert!(matches!(
            &app.event.reply,
            ReplyAddress::Telegram {
                thread_id: None,
                ..
            }
        ));
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        let caps = app
            .state
            .topic_capabilities(caller(&app), &app.event, &channel)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(caps).unwrap(),
            json!({"manage_original_topic":true,"actions":["close","reopen"],"expires_at":app.event.expires_at})
        );
        for action in ["close", "reopen"] {
            let expected = body(app.manage(app.target(), action, action).await).await;
            assert_eq!(expected, json!({"outcome":{"status":"confirmed"}}));
            let calls = telegram.0.lock().unwrap().calls.len();
            assert_eq!(
                body(app.manage(app.target(), action, action).await).await,
                expected
            );
            assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
        }
        assert_eq!(
            body(rename(&app, "Incident triage", "rename").await).await,
            json!({"outcome":{"status":"confirmed"}})
        );
        let calls = telegram.0.lock().unwrap().calls.len();
        assert_eq!(
            body(rename(&app, "Incident triage", "rename").await).await["outcome"]["status"],
            "confirmed"
        );
        assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
        assert_eq!(rename(&app, "Different name", "rename").await.status(), 409);
        assert_eq!(
            telegram.writes(),
            vec![
                ("closeGeneralForumTopic".into(), json!({"chat_id":"-100"})),
                ("reopenGeneralForumTopic".into(), json!({"chat_id":"-100"})),
                (
                    "editGeneralForumTopic".into(),
                    json!({"chat_id":"-100","name":"Incident triage"})
                ),
            ]
        );
        assert!(app.activity.conversation.thread_id.is_none());
        assert!(matches!(
            &app.event.reply,
            ReplyAddress::Telegram {
                thread_id: None,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn general_omission_alone_nonforum_and_synthetic_topic_one_never_grant_authority() {
    for defect in [
        "missing_forum",
        "not_forum",
        "string_forum",
        "private",
        "synthetic_one",
        "null_thread",
        "numbered_without_marker",
        "topic_marker",
        "false_marker",
        "direct_topic",
    ] {
        let mut raw = general_message();
        match defect {
            "missing_forum" => {
                raw["message"]["chat"]
                    .as_object_mut()
                    .unwrap()
                    .remove("is_forum");
            }
            "not_forum" => raw["message"]["chat"]["is_forum"] = false.into(),
            "string_forum" => raw["message"]["chat"]["is_forum"] = "true".into(),
            "private" => raw["message"]["chat"] = json!({"id":51,"type":"private","is_forum":true}),
            "synthetic_one" => {
                raw["message"]["message_thread_id"] = 1.into();
                raw["message"]["is_topic_message"] = true.into();
            }
            "null_thread" => raw["message"]["message_thread_id"] = Value::Null,
            "numbered_without_marker" => raw["message"]["message_thread_id"] = 123.into(),
            "topic_marker" => raw["message"]["is_topic_message"] = true.into(),
            "false_marker" => raw["message"]["is_topic_message"] = false.into(),
            "direct_topic" => raw["message"]["direct_messages_topic"] = json!({"topic_id":1}),
            _ => unreachable!(),
        }
        let (app, telegram) = fixture_with_native_message(false, true, raw, true).await;
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        assert!(
            app.state
                .topic_capabilities(caller(&app), &app.event, &channel)
                .await
                .is_none(),
            "{defect}"
        );
        assert!(
            !app.manage(app.target(), "close", defect)
                .await
                .status()
                .is_success(),
            "{defect}"
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty(), "{defect}");
    }
}

#[tokio::test]
async fn general_gate_and_original_source_consent_are_required_without_retroactive_authority() {
    for case in ["general_off", "all_topics_off", "late_consent"] {
        let (mut app, telegram) =
            fixture_with_native_message(false, case != "late_consent", general_message(), true)
                .await;
        match case {
            "general_off" => app.state.telegram_topics = Some(client(&telegram, false)),
            "all_topics_off" => app.state.telegram_topics = None,
            "late_consent" => {
                let Source::Telegram {
                    topic_lifecycle, ..
                } = &mut app.definition.sources[0]
                else {
                    panic!()
                };
                *topic_lifecycle = true;
                app.save();
            }
            _ => unreachable!(),
        }
        app.mount().await;
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        assert!(
            app.state
                .topic_capabilities(caller(&app), &app.event, &channel)
                .await
                .is_none(),
            "{case}"
        );
        assert!(
            !app.manage(app.target(), "close", case)
                .await
                .status()
                .is_success(),
            "{case}"
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty(), "{case}");
    }
}

#[tokio::test]
async fn general_stale_owner_consumer_event_and_forged_thread_fail_before_provider_io() {
    for change in ["owner", "consumer", "event", "expired", "thread"] {
        let (mut app, telegram) = general(true).await;
        match change {
            "owner" => {
                app.definition.owner.subject = "other-owner".into();
                app.save();
            }
            "consumer" => app.event.consumer = None,
            "event" => app.event.event_id = cmaeg_core::ids::EventId::new("other-event").unwrap(),
            "expired" => app.event.expires_at = now() - 1,
            "thread" => {
                let ReplyAddress::Telegram { thread_id, .. } = &mut app.event.reply else {
                    panic!()
                };
                *thread_id = Some("1".into());
            }
            _ => unreachable!(),
        }
        assert!(
            !app.manage(app.target(), "close", change)
                .await
                .status()
                .is_success(),
            "{change}"
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty(), "{change}");
    }
}

#[tokio::test]
async fn general_rechecks_current_source_and_credentials_after_platform_permission_reads() {
    for change in ["consent", "bot_token", "consumer_key", "provider_key"] {
        let (app, telegram) = general(false).await;
        let mut definition = app.definition.clone();
        match change {
            "consent" => {
                let Source::Telegram {
                    topic_lifecycle, ..
                } = &mut definition.sources[0]
                else {
                    panic!()
                };
                *topic_lifecycle = false;
            }
            "bot_token" => definition.secrets.telegram_bot_token = Some(Secret::new("77:rotated")),
            "consumer_key" => {
                definition.secrets.agent_key_hash =
                    crate::channels::definition::hash_secret("other-key")
            }
            "provider_key" => crate::channels::definition::fixture::replace_default_credential(
                &mut definition,
                Some("rotated-provider"),
            ),
            _ => unreachable!(),
        }
        let backend = app.backend.clone();
        let keys = app.state.keys.clone();
        telegram.0.lock().unwrap().hook = Some(Box::new(move || {
            backend.seed(
                definition.record_id.as_str(),
                &definition.owner.subject,
                seal_blob(
                    &keys,
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
            );
        }));
        assert_eq!(
            body(app.manage(app.target(), "close", change).await).await["outcome"]["status"],
            "rejected",
            "{change}"
        );
        assert!(telegram.writes().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn general_unknown_write_retains_canonical_fence_across_keys_restart_and_expiry() {
    let (mut app, telegram) = general(false).await;
    telegram
        .0
        .lock()
        .unwrap()
        .overrides
        .insert("closeGeneralForumTopic".into(), (500, json!({"ok":false})));
    let first = body(app.manage(app.target(), "close", "unknown").await).await;
    assert_eq!(first["outcome"]["status"], "unknown");
    telegram.0.lock().unwrap().overrides.clear();
    app.state.telegram_topics = Some(client(&telegram, true));
    app.mount().await;
    assert_eq!(
        body(app.manage(app.target(), "close", "unknown").await).await,
        first
    );
    assert_eq!(
        body(rename(&app, "Do not retry", "new-key").await).await["outcome"]["status"],
        "unknown"
    );
    assert_eq!(telegram.writes().len(), 1);
    app.backend.expire_at(app.event.expires_at + 1);
    let store = ledger::Store::telegram(app.state.resolver.record_client(), app.state.keys.clone());
    assert!(store
        .acquire_topic("77", "-100", "1", &app.definition.owner.subject)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .acquire_topic("77", "-200", "1", &app.definition.owner.subject)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .acquire_topic("77", "-100", "123", &app.definition.owner.subject)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn general_mcp_rename_and_rest_share_one_confirmed_operation() {
    let (app, telegram) = general(true).await;
    let response = reqwest::Client::new().post(format!("{}/mcp", app.base)).bearer_auth(&app.key)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"manageThread","arguments":{"request":{"target":app.target(),"action":"rename","name":"General triage"},"idempotency_key":"mcp-general"}}}))
        .send().await.unwrap();
    let result = body(response).await;
    assert!(result.to_string().contains("confirmed"), "{result}");
    let calls = telegram.0.lock().unwrap().calls.len();
    assert_eq!(
        body(rename(&app, "General triage", "mcp-general").await).await["outcome"]["status"],
        "confirmed"
    );
    assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
    assert_eq!(
        telegram.writes(),
        vec![(
            "editGeneralForumTopic".into(),
            json!({"chat_id":"-100","name":"General triage"})
        )]
    );
}

#[tokio::test]
async fn general_current_forum_and_administrator_permissions_are_required() {
    for (method, result) in [
        (
            "getChat",
            json!({"id":-100,"type":"supergroup","is_forum":false}),
        ),
        (
            "getChat",
            json!({"id":-200,"type":"supergroup","is_forum":true}),
        ),
        (
            "getChatMember",
            json!({"user":{"id":77,"is_bot":true},"status":"member"}),
        ),
        (
            "getChatMember",
            json!({"user":{"id":77,"is_bot":true},"status":"administrator","can_manage_topics":false}),
        ),
    ] {
        let (app, telegram) = general(false).await;
        telegram
            .0
            .lock()
            .unwrap()
            .overrides
            .insert(method.into(), (200, json!({"ok":true,"result":result})));
        assert_eq!(
            body(app.manage(app.target(), "close", "not-permitted").await).await["outcome"]
                ["status"],
            "rejected"
        );
        assert!(telegram.writes().is_empty());
        telegram.0.lock().unwrap().overrides.clear();
        assert_eq!(
            body(app.manage(app.target(), "close", "permitted").await).await["outcome"]["status"],
            "confirmed"
        );
        assert_eq!(telegram.writes().len(), 1);
    }
}
