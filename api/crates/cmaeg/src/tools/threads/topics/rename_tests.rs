use super::*;

async fn request(app: &App, value: Value, key: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/thread-operations", app.base))
        .bearer_auth(&app.key)
        .header("idempotency-key", key)
        .json(&value)
        .send()
        .await
        .unwrap()
}
async fn rename(app: &App, name: &str, key: &str) -> reqwest::Response {
    request(
        app,
        json!({"target":app.target(),"action":"rename","name":name}),
        key,
    )
    .await
}

#[test]
fn legacy_topic_action_bytes_hashes_and_strict_capabilities_are_unchanged() {
    for (old, new) in [
        (TopicAction::Close, Mutation::Close),
        (TopicAction::Reopen, Mutation::Reopen),
    ] {
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&new).unwrap()
        );
        assert_eq!(hash(&old), hash(&new));
    }
    let legacy =
        r#"{"manage_original_topic":true,"actions":["close","reopen"],"expires_at":1800000600}"#;
    let caps: TopicLifecycleCapabilities = serde_json::from_str(legacy).unwrap();
    assert_eq!(serde_json::to_string(&caps).unwrap(), legacy);
    assert!(serde_json::from_str::<TopicLifecycleCapabilities>(
        &legacy.replace("reopen", "rename")
    )
    .is_err());
}

#[tokio::test]
async fn exact_rename_preserves_topic_state_and_replays_with_name_conflicts() {
    for named in [false, true] {
        let (mut app, telegram) = fixture(named, -100, 123).await;
        let result = body(rename(&app, "Incident triage", "rename").await).await;
        assert_eq!(result, json!({"outcome":{"status":"confirmed"}}));
        let calls = telegram.0.lock().unwrap().calls.len();
        assert_eq!(
            body(rename(&app, "Incident triage", "rename").await).await,
            result
        );
        assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
        assert_eq!(rename(&app, "Different name", "rename").await.status(), 409);
        assert_eq!(
            app.manage(app.target(), "close", "rename").await.status(),
            409
        );
        assert_eq!(
            telegram.writes(),
            vec![(
                "editForumTopic".into(),
                json!({"chat_id":"-100","message_thread_id":123,"name":"Incident triage"})
            )]
        );
        assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
        let Source::Telegram {
            topic_lifecycle, ..
        } = &mut app.definition.sources[0]
        else {
            panic!()
        };
        *topic_lifecycle = false;
        app.save();
        assert!(!rename(&app, "Incident triage", "rename")
            .await
            .status()
            .is_success());
        assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
    }
}

#[tokio::test]
async fn rename_validates_name_and_refuses_raw_destinations_before_platform_io() {
    let (app, telegram) = fixture(false, -100, 123).await;
    for fields in [
        json!({"action":"rename"}),
        json!({"action":"rename","name":null}),
        json!({"action":"rename","name":""}),
        json!({"action":"rename","name":"   "}),
        json!({"action":"rename","name":"bad\u{001b}"}),
        json!({"action":"rename","name":"bad\u{202e}"}),
        json!({"action":"rename","name":"🧵".repeat(101)}),
        json!({"action":"rename","name":"Fine","chat_id":"-200"}),
        json!({"action":"rename","name":"Fine","message_thread_id":124}),
        json!({"action":"rename","name":"Fine","icon_custom_emoji_id":"123"}),
    ] {
        let mut value = fields;
        value["target"] = app.target();
        assert_eq!(
            request(&app, value.clone(), "bad").await.status(),
            400,
            "{value}"
        );
    }
    assert!(telegram.0.lock().unwrap().calls.is_empty());
    let name = "🧵".repeat(100);
    assert_eq!(
        body(rename(&app, &name, "valid").await).await["outcome"]["status"],
        "confirmed"
    );
    assert_eq!(telegram.writes()[0].1["name"], name);
}

#[tokio::test]
async fn rename_requires_original_native_topic_and_current_custody() {
    let (mut app, telegram) = fixture_with_consent(false, -100, 123, false).await;
    let Source::Telegram {
        topic_lifecycle, ..
    } = &mut app.definition.sources[0]
    else {
        panic!()
    };
    *topic_lifecycle = true;
    app.save();
    assert!(!rename(&app, "No retroactive authority", "later-consent")
        .await
        .status()
        .is_success());
    assert!(telegram.0.lock().unwrap().calls.is_empty());
    for (chat, topic) in [(51, 123), (-100, 1)] {
        let (app, telegram) = fixture(false, chat, topic).await;
        assert!(!rename(&app, "No authority", "scope")
            .await
            .status()
            .is_success());
        assert!(telegram.0.lock().unwrap().calls.is_empty());
    }
    for defect in ["source", "consent", "owner", "consumer", "topic", "expired"] {
        let (mut app, telegram) = fixture(true, -100, 123).await;
        match defect {
            "source" => app.definition.sources.clear(),
            "consent" => {
                let Source::Telegram {
                    topic_lifecycle, ..
                } = &mut app.definition.sources[0]
                else {
                    panic!()
                };
                *topic_lifecycle = false;
            }
            "owner" => app.definition.owner.subject = "other-owner".into(),
            "consumer" => app.event.consumer = None,
            "topic" => {
                let ReplyAddress::Telegram { thread_id, .. } = &mut app.event.reply else {
                    panic!()
                };
                *thread_id = Some("124".into());
            }
            "expired" => app.event.expires_at = now() - 1,
            _ => unreachable!(),
        }
        app.save();
        assert!(
            !rename(&app, "No authority", defect)
                .await
                .status()
                .is_success(),
            "{defect}"
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty(), "{defect}");
    }
}

#[tokio::test]
async fn rename_rechecks_consent_bot_token_and_consumer_credentials_after_permissions() {
    for change in ["consent", "bot_token", "consumer_key", "provider_key"] {
        let (app, telegram) = fixture(false, -100, 123).await;
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
            body(rename(&app, "Refused", change).await).await["outcome"]["status"],
            "rejected"
        );
        assert!(telegram.writes().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn rename_preflight_permissions_and_bot_identity_cannot_be_bypassed() {
    for (method, result) in [
        ("getMe", json!({"id":78,"is_bot":true,"first_name":"Other"})),
        (
            "getChat",
            json!({"id":-100,"type":"supergroup","is_forum":false}),
        ),
        (
            "getChatMember",
            json!({"user":{"id":77,"is_bot":true},"status":"member"}),
        ),
        (
            "getChatMember",
            json!({"user":{"id":77,"is_bot":true},"status":"administrator","can_manage_topics":false}),
        ),
        (
            "getChatMember",
            json!({"user":{"id":78,"is_bot":true},"status":"administrator","can_manage_topics":true}),
        ),
    ] {
        let (app, telegram) = fixture(false, -100, 123).await;
        telegram
            .0
            .lock()
            .unwrap()
            .overrides
            .insert(method.into(), (200, json!({"ok":true,"result":result})));
        assert_eq!(
            body(rename(&app, "Refused", "refused").await).await["outcome"]["status"],
            "rejected"
        );
        assert!(telegram.writes().is_empty());
        telegram.0.lock().unwrap().overrides.clear();
        assert_eq!(
            body(rename(&app, "Permitted", "permitted").await).await["outcome"]["status"],
            "confirmed"
        );
        assert_eq!(telegram.writes().len(), 1);
    }
}

#[tokio::test]
async fn rename_uncertainty_fences_all_actions_across_new_keys_and_restart() {
    for failure in ["server", "nontrue", "timeout", "lost_receipt"] {
        let (mut app, telegram) = fixture(false, -100, 123).await;
        match failure {
            "server" => {
                telegram
                    .0
                    .lock()
                    .unwrap()
                    .overrides
                    .insert("editForumTopic".into(), (500, json!({"ok":false})));
            }
            "nontrue" => {
                telegram.0.lock().unwrap().overrides.insert(
                    "editForumTopic".into(),
                    (200, json!({"ok":true,"result":false})),
                );
            }
            "timeout" => {
                let mut client = TelegramTopics::new(Arc::new(
                    TelegramClient::production(Duration::from_secs(1))
                        .unwrap()
                        .with_transport(telegram.clone()),
                ));
                client.budget = Duration::from_millis(400);
                app.state.telegram_topics = Some(Arc::new(client));
                telegram.0.lock().unwrap().delay = Duration::from_secs(1);
                app.mount().await;
            }
            "lost_receipt" => {
                let fail = app.backend.fail_put.clone();
                telegram.0.lock().unwrap().write_hook = Some(Box::new(move || {
                    fail.store(true, std::sync::atomic::Ordering::SeqCst)
                }));
            }
            _ => unreachable!(),
        }
        let first = body(rename(&app, "Uncertain", "first").await).await;
        assert_eq!(first["outcome"]["status"], "unknown", "{failure}");
        app.backend
            .fail_put
            .store(false, std::sync::atomic::Ordering::SeqCst);
        telegram.0.lock().unwrap().overrides.clear();
        telegram.0.lock().unwrap().delay = Duration::ZERO;
        app.state.telegram_topics = Some(Arc::new(TelegramTopics::new(Arc::new(
            TelegramClient::production(Duration::from_secs(1))
                .unwrap()
                .with_transport(telegram.clone()),
        ))));
        app.mount().await;
        let calls = telegram.0.lock().unwrap().calls.len();
        assert_eq!(
            body(rename(&app, "Uncertain", "first").await).await["outcome"]["status"],
            "unknown"
        );
        assert_eq!(
            body(rename(&app, "Changed", "another").await).await["outcome"]["status"],
            "unknown"
        );
        for action in ["close", "reopen"] {
            assert_eq!(
                body(app.manage(app.target(), action, action).await).await["outcome"]["status"],
                "unknown"
            );
        }
        assert_eq!(telegram.0.lock().unwrap().calls.len(), calls, "{failure}");
        assert_eq!(telegram.writes().len(), 1, "{failure}");
    }
}

#[tokio::test]
async fn rename_shares_existing_close_fence_and_mcp_rest_replay() {
    let (app, telegram) = fixture(true, -100, 123).await;
    let store = ledger::Store::telegram(app.state.resolver.record_client(), app.state.keys.clone());
    let held = store
        .acquire_topic("77", "-100", "123", &app.definition.owner.subject)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        body(rename(&app, "Blocked", "held").await).await["outcome"]["status"],
        "unknown"
    );
    assert!(telegram.0.lock().unwrap().calls.is_empty());
    store.release(held).await.unwrap();
    let response = reqwest::Client::new().post(format!("{}/mcp", app.base)).bearer_auth(&app.key)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"manageThread","arguments":{"request":{"target":app.target(),"action":"rename","name":"Renamed"},"idempotency_key":"mcp-rename"}}}))
        .send().await.unwrap();
    let result = body(response).await;
    assert!(result.to_string().contains("confirmed"), "{result}");
    let calls = telegram.0.lock().unwrap().calls.len();
    assert_eq!(
        body(rename(&app, "Renamed", "mcp-rename").await).await["outcome"]["status"],
        "confirmed"
    );
    assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
    assert_eq!(telegram.writes().len(), 1);
}
