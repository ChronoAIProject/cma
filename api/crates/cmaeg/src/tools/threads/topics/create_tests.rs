use super::*;

fn message(numbered: bool) -> Value {
    let mut raw = json!({"update_id":101,"message":{"message_id":800,"date":now()-5,
        "chat":{"id":-100,"type":"supergroup","is_forum":true},
        "from":{"id":51,"is_bot":false,"first_name":"Alice"},"text":"Please create a topic"}});
    if numbered {
        raw["message"]["message_thread_id"] = 123.into();
        raw["message"]["is_topic_message"] = true.into();
    }
    raw
}
async fn fixture_create(named: bool, numbered: bool) -> (App, Arc<Telegram>) {
    fixture_with_grants(named, false, true, message(numbered), false).await
}
async fn create(app: &App, request: Value, key: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/v1/events/{}/threads",
            app.base,
            app.reference().as_str()
        ))
        .bearer_auth(&app.key)
        .header("Idempotency-Key", key)
        .json(&request)
        .send()
        .await
        .unwrap()
}
fn client(telegram: &Arc<Telegram>) -> TelegramTopics {
    TelegramTopics::new(Arc::new(
        TelegramClient::production(Duration::from_secs(1))
            .unwrap()
            .with_transport(telegram.clone()),
    ))
    .with_lifecycle(false)
    .with_creation(true)
}
fn replace(app: &App, definition: &cmaeg_core::channel::ChannelDefinition) {
    app.backend.seed(
        definition.record_id.as_str(),
        &definition.owner.subject,
        seal_blob(
            &app.state.keys,
            &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
        )
        .unwrap(),
    );
}

#[tokio::test]
async fn creates_in_original_forum_and_returns_usable_scoped_reference_without_other_topic_consent()
{
    for named in [false, true] {
        for numbered in [false, true] {
            let (mut app, telegram) = fixture_create(named, numbered).await;
            app.state.telegram_topics = Some(Arc::new(client(&telegram)));
            app.mount().await;
            assert_eq!(
                app.state
                    .topic_creation_capability(caller(&app), &app.event)
                    .await,
                Some(true)
            );
            let original = serde_json::to_value(&app.event.reply).unwrap();
            let result =
                body(create(&app, json!({"name":"Project planning"}), "create").await).await;
            assert_eq!(result["outcome"]["status"], "confirmed");
            assert_eq!(result["thread"]["thread_id"], "901");
            assert_eq!(result["thread"]["parent_id"], "-100");
            assert_eq!(result["thread"]["name"], "Project planning");
            assert_eq!(result["thread"]["expires_at"], app.event.expires_at);
            let reference: ThreadRef =
                serde_json::from_value(result["thread"]["thread_ref"].clone()).unwrap();
            let opened = reference.open_telegram(&app.state.keys, now()).unwrap();
            assert_eq!(opened.payload.event.event_id, app.event.event_id);
            assert!(reference.open(&app.state.keys, now()).is_err());
            let calls = telegram.0.lock().unwrap().calls.len();
            assert_eq!(
                body(create(&app, json!({"name":"Project planning"}), "create").await).await,
                result
            );
            assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
            assert_eq!(
                create(&app, json!({"name":"Different"}), "create")
                    .await
                    .status(),
                409
            );
            let target = json!({"type":"thread","thread_ref":reference});
            for action in ["close", "reopen"] {
                assert_eq!(
                    body(app.manage(target.clone(), action, action).await).await["outcome"]
                        ["status"],
                    "confirmed"
                );
            }
            let response = reqwest::Client::new()
                .post(format!("{}/v1/thread-operations", app.base))
                .bearer_auth(&app.key)
                .header("Idempotency-Key", "rename-created")
                .json(&json!({"target":target,"action":"rename","name":"Release planning"}))
                .send()
                .await
                .unwrap();
            assert_eq!(body(response).await["outcome"]["status"], "confirmed");
            assert_eq!(
                telegram.writes(),
                vec![
                    (
                        "createForumTopic".into(),
                        json!({"chat_id":"-100","name":"Project planning"})
                    ),
                    (
                        "closeForumTopic".into(),
                        json!({"chat_id":"-100","message_thread_id":901})
                    ),
                    (
                        "reopenForumTopic".into(),
                        json!({"chat_id":"-100","message_thread_id":901})
                    ),
                    (
                        "editForumTopic".into(),
                        json!({"chat_id":"-100","message_thread_id":901,"name":"Release planning"})
                    ),
                ]
            );
            assert_eq!(serde_json::to_value(&app.event.reply).unwrap(), original);
            assert!(!app
                .manage(app.target(), "close", "original-not-consented")
                .await
                .status()
                .is_success());
            assert!(!app.list("active").await.status().is_success());
        }
    }
}

#[tokio::test]
async fn telegram_creation_validates_neutral_request_before_any_platform_io() {
    let (app, telegram) = fixture_create(false, true).await;
    for request in [
        json!({"name":""}),
        json!({"name":"  "}),
        json!({"name":"x".repeat(101)}),
        json!({"name":"bad\nname"}),
        json!({"name":"Review","auto_archive_duration":1440}),
        json!({"name":"Review","auto_archive_duration":null}),
        json!({"name":"Review","chat_id":"-200"}),
        json!({"name":"Review","icon_color":7322096}),
        json!({"name":"Review","message_thread_id":123}),
    ] {
        assert_eq!(create(&app, request, "invalid").await.status(), 400);
    }
    assert!(telegram.0.lock().unwrap().calls.is_empty());
    assert_eq!(
        body(create(&app, json!({"name":"界".repeat(100)}), "valid").await).await["outcome"]
            ["status"],
        "confirmed"
    );
}

#[tokio::test]
async fn original_forum_proof_and_creation_consent_cannot_be_inferred_or_added_retroactively() {
    for case in [
        "no_forum",
        "false_forum",
        "private",
        "late_consent",
        "gate_off",
        "owner",
        "consumer",
        "expired",
    ] {
        let mut raw = message(false);
        match case {
            "no_forum" => {
                raw["message"]["chat"]
                    .as_object_mut()
                    .unwrap()
                    .remove("is_forum");
            }
            "false_forum" => raw["message"]["chat"]["is_forum"] = false.into(),
            "private" => raw["message"]["chat"] = json!({"id":51,"type":"private","is_forum":true}),
            _ => (),
        }
        let (mut app, telegram) =
            fixture_with_grants(true, false, case != "late_consent", raw, false).await;
        match case {
            "late_consent" => {
                let Source::Telegram { topic_creation, .. } = &mut app.definition.sources[0] else {
                    panic!()
                };
                *topic_creation = true;
                app.save();
                app.state.telegram_topics = Some(Arc::new(client(&telegram)));
            }
            "gate_off" => {
                app.state.telegram_topics = Some(Arc::new(client(&telegram).with_creation(false)))
            }
            "owner" => {
                app.definition.owner.subject = "different-owner".into();
                app.save();
            }
            "consumer" => app.event.consumer = None,
            "expired" => app.event.expires_at = now() - 1,
            _ => (),
        }
        app.mount().await;
        assert!(
            app.state
                .topic_creation_capability(caller(&app), &app.event)
                .await
                .is_none(),
            "{case}"
        );
        assert!(
            !create(&app, json!({"name":"Refused"}), case)
                .await
                .status()
                .is_success(),
            "{case}"
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty(), "{case}");
    }
}

#[tokio::test]
async fn creation_requires_current_forum_bot_identity_and_topic_permission() {
    for (method, value) in [
        ("getMe", json!({"id":88,"is_bot":true,"first_name":"Other"})),
        (
            "getChat",
            json!({"id":-200,"type":"supergroup","is_forum":true}),
        ),
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
    ] {
        let (app, telegram) = fixture_create(false, true).await;
        telegram
            .0
            .lock()
            .unwrap()
            .overrides
            .insert(method.into(), (200, json!({"ok":true,"result":value})));
        assert_eq!(
            body(create(&app, json!({"name":"Denied"}), "denied").await).await["outcome"]["status"],
            "rejected"
        );
        assert!(telegram.writes().is_empty());
        telegram.0.lock().unwrap().overrides.clear();
        assert_eq!(
            body(create(&app, json!({"name":"Allowed"}), "allowed").await).await["outcome"]
                ["status"],
            "confirmed"
        );
    }
}

#[tokio::test]
async fn current_custody_is_rechecked_before_creation_and_created_topic_management() {
    for change in ["consent", "bot_token", "consumer_key", "provider_key"] {
        let (app, telegram) = fixture_create(false, true).await;
        let result = body(create(&app, json!({"name":"Created"}), "created").await).await;
        let target = json!({"type":"thread","thread_ref":result["thread"]["thread_ref"]});
        let mut definition = app.definition.clone();
        match change {
            "consent" => {
                let Source::Telegram { topic_creation, .. } = &mut definition.sources[0] else {
                    panic!()
                };
                *topic_creation = false;
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
        let changed = definition.clone();
        telegram.0.lock().unwrap().hook = Some(Box::new(move || {
            backend.seed(
                changed.record_id.as_str(),
                &changed.owner.subject,
                seal_blob(&keys, &serde_json::to_vec(&changed.to_blob_json()).unwrap()).unwrap(),
            );
        }));
        assert_eq!(
            body(create(&app, json!({"name":"Revoked"}), "revoked").await).await["outcome"]
                ["status"],
            "rejected",
            "{change}"
        );
        assert_eq!(telegram.writes().len(), 1);
        replace(&app, &definition);
        assert!(
            !app.manage(target, "close", "changed-custody")
                .await
                .status()
                .is_success(),
            "{change}"
        );
        assert_eq!(telegram.writes().len(), 1);
    }
}

#[tokio::test]
async fn uncertain_creation_keeps_chat_fence_across_keys_restart_and_event_expiry() {
    for failure in ["upstream", "malformed", "timeout", "lost_receipt"] {
        let (mut app, telegram) = fixture_create(false, true).await;
        match failure {
            "upstream" => {
                telegram
                    .0
                    .lock()
                    .unwrap()
                    .overrides
                    .insert("createForumTopic".into(), (500, json!({"ok":false})));
            }
            "malformed" => {
                telegram.0.lock().unwrap().overrides.insert("createForumTopic".into(),(200,json!({"ok":true,"result":{"message_thread_id":1,"name":"Uncertain","icon_color":1}})));
            }
            "timeout" => {
                let mut client = client(&telegram);
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
        assert_eq!(
            body(create(&app, json!({"name":"Uncertain"}), "first").await).await["outcome"]
                ["status"],
            "unknown",
            "{failure}"
        );
        app.backend
            .fail_put
            .store(false, std::sync::atomic::Ordering::SeqCst);
        telegram.0.lock().unwrap().overrides.clear();
        telegram.0.lock().unwrap().delay = Duration::ZERO;
        app.state.telegram_topics = Some(Arc::new(client(&telegram)));
        app.mount().await;
        let calls = telegram.0.lock().unwrap().calls.len();
        for (key, name) in [("first", "Uncertain"), ("new-key", "Different")] {
            assert_eq!(
                body(create(&app, json!({"name":name}), key).await).await["outcome"]["status"],
                "unknown"
            );
        }
        assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
        assert_eq!(telegram.writes().len(), 1);
        app.backend.expire_at(app.event.expires_at + 1);
        let store =
            ledger::Store::telegram(app.state.resolver.record_client(), app.state.keys.clone());
        assert!(store
            .acquire_topic_creation("77", "-100", &app.definition.owner.subject)
            .await
            .unwrap()
            .is_none());
        assert!(store
            .acquire_topic_creation("77", "-200", &app.definition.owner.subject)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .acquire_topic("77", "-100", "123", &app.definition.owner.subject)
            .await
            .unwrap()
            .is_some());
    }
}

#[tokio::test]
async fn telegram_only_mcp_advertises_creation_and_shares_the_rest_receipt() {
    let (app, telegram) = fixture_create(true, false).await;
    let endpoint = format!("{}/mcp", app.base);
    let listed = body(
        reqwest::Client::new()
            .post(&endpoint)
            .bearer_auth(&app.key)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"createEventThread"));
    assert!(names.contains(&"manageThread"));
    assert!(!names.contains(&"listEventThreads"));
    let called=body(reqwest::Client::new().post(&endpoint).bearer_auth(&app.key)
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"createEventThread","arguments":{"event_ref":app.reference(),"request":{"name":"MCP topic"},"idempotency_key":"mcp"}}})).send().await.unwrap()).await;
    assert!(called.to_string().contains("confirmed"), "{called}");
    let calls = telegram.0.lock().unwrap().calls.len();
    assert_eq!(
        body(create(&app, json!({"name":"MCP topic"}), "mcp").await).await["outcome"]["status"],
        "confirmed"
    );
    assert_eq!(telegram.0.lock().unwrap().calls.len(), calls);
    assert_eq!(telegram.writes().len(), 1);
}

#[tokio::test]
async fn concurrent_and_cancelled_creation_cannot_dispatch_a_second_topic() {
    let (app, telegram) = fixture_create(true, false).await;
    let dispatched = Arc::new(tokio::sync::Notify::new());
    let signal = dispatched.clone();
    telegram.0.lock().unwrap().write_hook = Some(Box::new(move || signal.notify_one()));
    telegram.0.lock().unwrap().delay = Duration::from_secs(10);
    let state = app.state.clone();
    let reference = app.reference();
    let caller = caller(&app);
    let first = tokio::spawn(async move {
        state
            .create_event_thread(
                caller,
                &reference,
                CreateThreadRequest {
                    name: "One topic".into(),
                    auto_archive_duration: None,
                },
                "first",
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), dispatched.notified())
        .await
        .unwrap();
    assert_eq!(
        body(create(&app, json!({"name":"Concurrent topic"}), "second").await).await["outcome"]
            ["status"],
        "unknown"
    );
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    telegram.0.lock().unwrap().delay = Duration::ZERO;
    for (key, name) in [("first", "One topic"), ("third", "Another topic")] {
        assert_eq!(
            body(create(&app, json!({"name":name}), key).await).await["outcome"]["status"],
            "unknown"
        );
    }
    assert_eq!(telegram.writes().len(), 1);
}

#[tokio::test]
async fn confirmed_creation_after_custody_revocation_does_not_issue_fresh_authority() {
    let (app, telegram) = fixture_create(true, true).await;
    let mut definition = app.definition.clone();
    let Source::Telegram { topic_creation, .. } = &mut definition.sources[0] else {
        panic!()
    };
    *topic_creation = false;
    let backend = app.backend.clone();
    let keys = app.state.keys.clone();
    telegram.0.lock().unwrap().write_hook = Some(Box::new(move || {
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
    let result =
        body(create(&app, json!({"name":"Created before revocation"}), "create").await).await;
    assert_eq!(result["outcome"]["status"], "confirmed");
    assert!(result.get("thread").is_none());
    assert_eq!(telegram.writes().len(), 1);
}
