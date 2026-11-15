use super::*;

#[tokio::test]
async fn thread_context_reads_only_on_request_and_retains_chronological_edited_bot_text() {
    let app = App::new().await;
    let ordinary: Value = app.read("").await.json().await.unwrap();
    assert_eq!(ordinary["capabilities"]["thread_history"], true);
    assert!(ordinary.get("thread_history").is_none());
    assert_eq!(app.platform_reads(), 0);
    app.memory.lock().unwrap().messages[0]["author"]["bot"] = true.into();
    app.memory.lock().unwrap().messages[0]["edited_timestamp"] = "2026-10-27T01:01:00Z".into();
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["thread_history"]["messages"][0]["message_id"], "698");
    assert_eq!(
        body["thread_history"]["messages"][1]["author"]["kind"],
        "bot"
    );
    assert_eq!(
        body["thread_history"]["messages"][1]["edited_at"],
        "2026-10-27T01:01:00Z"
    );
    assert_eq!(app.platform_reads(), 4);
    for (path, auth) in &app.memory.lock().unwrap().calls {
        if path.starts_with("/api/") {
            assert_eq!(auth, "Bot thread-history-fixture");
        }
    }
}

#[tokio::test]
async fn thread_context_refuses_unknown_query_caller_destinations_and_missing_gate_or_source_consent(
) {
    let mut app = App::new().await;
    for query in [
        "?include_thread_history=true&channel_id=88",
        "?include_thread_history=true&before=1",
        "?include_thread_history=invalid",
    ] {
        assert_eq!(app.read(query).await.status(), 400);
    }
    assert_eq!(app.platform_reads(), 0);
    app.state.discord_thread_history = None;
    app.mount().await;
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    let body: Value = app.read("").await.json().await.unwrap();
    assert!(body["capabilities"].get("thread_history").is_none());
    let mut app = App::new().await;
    let Source::Discord { thread_history, .. } = &mut app.definition.sources[0] else {
        panic!()
    };
    *thread_history = false;
    app.save();
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    assert_eq!(app.platform_reads(), 0);
}

#[tokio::test]
async fn thread_context_refuses_foreign_thread_missing_proof_and_stale_consumer_key() {
    for case in 0..4 {
        let mut app = App::new().await;
        match case {
            0 => {
                app.event.activity_sha256 = None;
            }
            1 => {
                if let ReplyAddress::Discord { thread_id, .. } = &mut app.event.reply {
                    *thread_id = None;
                }
            }
            2 => {
                fixture::replace_default_agent_key(
                    &mut app.definition,
                    "replacement-agent-fixture",
                );
                app.save();
            }
            _ => {
                app.memory.lock().unwrap().channel["id"] = "88".into();
            }
        }
        app.snapshot();
        assert!(
            !app.read("?include_thread_history=true")
                .await
                .status()
                .is_success(),
            "case {case}"
        );
        assert!(!app
            .memory
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(path, _)| path.contains("?before=")));
    }
}

#[tokio::test]
async fn thread_context_checks_authority_again_after_read_and_never_returns_revoked_data() {
    for case in 0..3 {
        let app = App::new().await;
        let backend = app.backend.clone();
        let keys = app.state.keys.clone();
        let mut changed = app.definition.clone();
        match case {
            0 => {
                changed.secrets.discord_bot_token =
                    Some(cmaeg_core::channel::Secret::new("rotated-thread-fixture"))
            }
            1 => {
                let Source::Discord { thread_history, .. } = &mut changed.sources[0] else {
                    panic!()
                };
                *thread_history = false;
            }
            _ => fixture::replace_default_agent_key(&mut changed, "revoked-agent-fixture"),
        }
        app.memory.lock().unwrap().on_history = Some(Box::new(move || {
            backend.seed(
                changed.record_id.as_str(),
                &changed.owner.subject,
                seal_blob(&keys, &serde_json::to_vec(&changed.to_blob_json()).unwrap()).unwrap(),
            )
        }));
        assert_eq!(
            app.read("?include_thread_history=true").await.status(),
            403,
            "case {case}"
        );
    }
}

#[tokio::test]
async fn thread_context_is_same_explicit_operation_over_mcp() {
    let app = App::new().await;
    let response=reqwest::Client::new().post(format!("{}/mcp",app.base)).bearer_auth(&app.key).json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"readEventContext","arguments":{"event_ref":app.reference(),"include_thread_history":true}}})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(body.to_string().contains("before_message_id"), "{body}");
    assert_eq!(app.platform_reads(), 4);
}

#[tokio::test]
async fn thread_context_refuses_oversized_cross_channel_future_duplicate_and_redirected_results() {
    for case in 0..6 {
        let app = App::new().await;
        {
            let mut memory = app.memory.lock().unwrap();
            match case {
                0 => {
                    memory.messages = json!((0..21)
                        .map(|id| history_message(&(600 + id).to_string()))
                        .collect::<Vec<_>>())
                }
                1 => memory.messages[0]["channel_id"] = "88".into(),
                2 => memory.messages[0]["id"] = "700".into(),
                3 => memory.messages[1]["id"] = "699".into(),
                4 => memory.status = StatusCode::TEMPORARY_REDIRECT,
                _ => memory.messages = Value::String("x".repeat(1024 * 1024)),
            }
        }
        assert!(
            !app.read("?include_thread_history=true")
                .await
                .status()
                .is_success(),
            "case {case}"
        );
        assert_eq!(app.platform_reads(), 4);
    }
}

#[tokio::test]
async fn thread_context_bounds_unicode_text_and_never_exports_attachment_authority() {
    let app = App::new().await;
    app.memory.lock().unwrap().messages = json!((690..696).map(|id| {
        let mut message = history_message(&id.to_string());
        message["content"] = format!("\u{1b}{}", "界".repeat(9000)).into();
        message["attachments"] = json!([{"id":"123", "url":"https://cdn.example/private-file", "proxy_url":"https://proxy.example/private-file"}]);
        message
    }).collect::<Vec<_>>());
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let history = &body["thread_history"];
    assert_eq!(history["text_truncated"], true);
    let messages = history["messages"].as_array().unwrap();
    let total: usize = messages
        .iter()
        .map(|message| {
            let text = message["text"].as_str().unwrap();
            assert!(text.len() <= 8192);
            assert!(!text.contains('\u{1b}'));
            assert_eq!(message["attachment_count"], 1);
            text.len()
        })
        .sum();
    assert!(total <= 32768 && total > 24000);
    assert!(!history.to_string().contains("private-file"));
    assert!(!history.to_string().contains("message_ref"));
}

#[tokio::test]
async fn thread_context_accepts_verified_thread_types_and_refuses_parent_or_missing_anchor() {
    for kind in [10, 11, 12, 0] {
        let app = App::new().await;
        app.memory.lock().unwrap().channel["type"] = kind.into();
        let status = app.read("?include_thread_history=true").await.status();
        assert_eq!(status, if kind == 0 { 403 } else { 200 }, "kind {kind}");
        assert_eq!(app.platform_reads(), if kind == 0 { 2 } else { 4 });
    }
    let app = App::new().await;
    app.memory.lock().unwrap().anchor_status = StatusCode::NOT_FOUND;
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    assert_eq!(app.platform_reads(), 3);
}

#[tokio::test]
async fn thread_context_rate_limit_is_not_retried_and_cools_down_following_reads() {
    let app = App::new().await;
    app.memory.lock().unwrap().status = StatusCode::TOO_MANY_REQUESTS;
    app.memory.lock().unwrap().messages = json!({"retry_after": 60});
    assert_eq!(app.read("?include_thread_history=true").await.status(), 429);
    assert_eq!(app.platform_reads(), 4);
    assert_eq!(app.read("?include_thread_history=true").await.status(), 429);
    assert_eq!(app.platform_reads(), 4);
}

#[tokio::test]
async fn thread_context_preserves_named_consumer_binding_and_rejects_default_key() {
    let mut app = App::new().await;
    let reviewer = "csm_00000000000a";
    let reviewer_key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
    let hash = crate::channels::definition::hash_secret(&reviewer_key);
    let default = app.definition.consumer(None).unwrap();
    app.definition.routing = Some(serde_json::from_value(json!({
        "version":1,"default_consumer":{"name":"lead"},"default_targets":["default"],
        "consumers":[{"id":reviewer,"name":"reviewer","profile":default.profile,
            "profile_digest":default.profile.digest(),
            "provider":{"slug":default.provider.slug,"binding_id":"binding-reviewer","level":"l1"},
            "agent_key_hash":hash,"created_at":"2026-10-01T00:00:00Z"}]
    })).unwrap());
    app.definition.secrets.consumer_keys = vec![(
        cmaeg_core::ids::ConsumerId::parse(reviewer).unwrap(),
        cmaeg_core::channel::Secret::new(&reviewer_key),
    )];
    app.definition.validate().unwrap();
    app.event.consumer = Some(cmaeg_core::ids::ConsumerId::parse(reviewer).unwrap());
    app.save();
    app.snapshot();
    assert_eq!(app.read("?include_thread_history=true").await.status(), 403);
    assert_eq!(app.platform_reads(), 0);
    app.key = reviewer_key.clone();
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["consumer"]["id"], reviewer);
    assert_eq!(body["provider_binding_id"], "binding-reviewer");
    let memory = app.memory.lock().unwrap();
    let reads: Vec<_> = memory
        .calls
        .iter()
        .filter(|(path, _)| path.starts_with("/provider/"))
        .collect();
    assert_eq!(reads.len(), 1);
    assert!(reads[0].0.starts_with("/provider/binding-reviewer/"));
    assert_eq!(reads[0].1, format!("Bearer {reviewer_key}"));
}

#[tokio::test]
async fn thread_context_refuses_expiry_during_the_final_authority_read() {
    let mut app = App::new().await;
    app.event.expires_at = time::OffsetDateTime::now_utc().unix_timestamp() + 2;
    app.snapshot();
    app.memory.lock().unwrap().history_release_at = Some(
        time::OffsetDateTime::from_unix_timestamp(app.event.expires_at).unwrap()
            - time::Duration::milliseconds(400),
    );
    let delay = app.backend.get_delay_ms.clone();
    app.memory.lock().unwrap().on_history = Some(Box::new(move || {
        delay.store(600, std::sync::atomic::Ordering::SeqCst);
    }));
    let response = app.read("?include_thread_history=true").await;
    assert_eq!(response.status(), 410);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "event_ref_expired");
    assert_eq!(app.platform_reads(), 4);
}
