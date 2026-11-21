//! Real upload issuance and public effects with process-local fake platform IO.
use super::*;

type Gate = (Arc<tokio::sync::Semaphore>, Arc<tokio::sync::Semaphore>);

#[derive(Default)]
struct MediaPlatform {
    calls: Mutex<Vec<(String, Value)>>,
    uncertain: AtomicBool,
    fail_receipt: Mutex<Option<Arc<AtomicBool>>>,
    gate: Mutex<Option<Gate>>,
}
impl TelegramTransport for MediaPlatform {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(url.host_str(), Some("api.telegram.org"));
        let method = url.path().rsplit('/').next().unwrap().to_owned();
        let upload = method == "sendDocument";
        let body = if upload {
            assert!(headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("multipart/form-data"));
            let body = String::from_utf8_lossy(&body);
            assert!(body.contains("name=\"document\""));
            Value::String(body.into_owned())
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        self.calls.lock().unwrap().push((method, body));
        Box::pin(async move {
            if !upload {
                let gate = self.gate.lock().unwrap().clone();
                if let Some((entered, release)) = gate {
                    entered.add_permits(1);
                    release.acquire().await.unwrap().forget();
                }
                if let Some(fail) = self.fail_receipt.lock().unwrap().as_ref() {
                    fail.store(true, Ordering::SeqCst);
                }
                if self.uncertain.load(Ordering::SeqCst) {
                    return Err(TransportError::Unavailable);
                }
            }
            Ok(HttpReply {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Ok(serde_json::to_vec(&if upload {
                    json!({"ok":true,"result":{"message_id":800,"chat":{"id":42}}})
                } else {
                    json!({"ok":true,"result":true})
                })
                .unwrap()),
            })
        })
    }
}
fn media() -> Value {
    json!({"native_attachments":[{"kind":"file","filename":"report.bin",
        "caption":"report", "source":{"type":"base64","data":"AQIDBA=="}}]})
}
async fn app() -> (App, Arc<MediaPlatform>) {
    let (mut app, _, _) = setup().await;
    let platform = Arc::new(MediaPlatform::default());
    let native = TelegramClient::production(Duration::from_secs(2))
        .unwrap()
        .with_transport(platform.clone());
    app.state.telegram_effects = Some(Arc::new(MessageEffectsClient::new(Arc::new(
        native.clone(),
    ))));
    app.native = Some(native);
    app.telegram_media_effects = true;
    app.restart().await;
    (app, platform)
}
async fn upload(app: &App, event: &cmaeg_core::seal::EventRef, key: &str) -> Value {
    receipt(
        app.client
            .post(format!("{}/v1/events/{}/replies", app.base, event.as_str()))
            .bearer_auth(key)
            .json(&media())
            .send()
            .await
            .unwrap(),
    )
    .await
}
async fn reference(app: &App) -> MessageRef {
    let event = app.event("42", "upload").await;
    let sent = upload(app, &event, &app.key).await;
    assert_eq!(sent["delivery"]["outcome"], "confirmed");
    let reference = MessageRef::new(sent["message_ref"].as_str().unwrap()).unwrap();
    assert!(
        reference
            .open(&app.state.keys)
            .unwrap()
            .payload
            .telegram_effect
            .unwrap()
            .effects_only
    );
    reference
}

#[tokio::test]
async fn telegram_media_effects_replay_after_restart_and_gate_off_without_text_edits() {
    let (mut app, platform) = app().await;
    let reference = reference(&app).await;
    app.telegram_media_effects = false;
    app.state.telegram_effects_issuance_enabled = false;
    app.restart().await;
    assert_eq!(capabilities(&app, &reference).await["delete"], true);
    assert_eq!(capabilities(&app, &reference).await["reaction"], true);
    assert_eq!(
        receipt(
            effect(
                &app,
                &reference,
                "update",
                "edit",
                json!({"text":"changed"})
            )
            .send()
            .await
            .unwrap()
        )
        .await["delivery"]["outcome"],
        "unsupported"
    );
    for (operation, key, body) in [
        (
            "reaction",
            "react",
            json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
        ),
        ("reaction", "clear", json!({"reaction":[]})),
        ("delete", "delete", json!({})),
    ] {
        let first = receipt(
            effect(&app, &reference, operation, key, body.clone())
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], "confirmed");
        app.restart().await;
        assert_eq!(
            receipt(
                effect(&app, &reference, operation, key, body)
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            first
        );
    }
    let calls = platform.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].0, "sendDocument");
    assert_eq!(calls[1].0, "setMessageReaction");
    assert_eq!(calls[2].1["reaction"], json!([]));
    assert_eq!(calls[3].0, "deleteMessage");
    for (_, body) in &calls[1..] {
        assert_eq!(body["chat_id"], "42");
        assert_eq!(body["message_id"], "800");
    }
}

#[tokio::test]
async fn telegram_media_unknown_or_lost_receipt_fences_new_keys_after_restart() {
    for lost_receipt in [false, true] {
        let (mut app, platform) = app().await;
        let reference = reference(&app).await;
        if lost_receipt {
            *platform.fail_receipt.lock().unwrap() = Some(app.backend.fail_put.clone());
        } else {
            platform.uncertain.store(true, Ordering::SeqCst);
        }
        let first = receipt(
            effect(&app, &reference, "delete", "first", json!({}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], "unknown");
        app.backend.fail_put.store(false, Ordering::SeqCst);
        app.restart().await;
        let replay = receipt(
            effect(&app, &reference, "delete", "first", json!({}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(replay["delivery"]["outcome"], "unknown");
        assert_eq!(
            receipt(
                effect(
                    &app,
                    &reference,
                    "reaction",
                    "second",
                    json!({"reaction":[]})
                )
                .send()
                .await
                .unwrap()
            )
            .await["delivery"]["reason"],
            "native_message_effect_busy"
        );
        assert_eq!(platform.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn telegram_media_effects_recheck_owner_source_provider_key_and_bot_custody() {
    for changed in ["owner", "source", "provider", "key", "bot", "policy"] {
        let (mut app, platform) = app().await;
        let reference = reference(&app).await;
        capabilities(&app, &reference).await;
        match changed {
            "owner" => app.definition.owner.subject = "new-owner".into(),
            "source" => app.definition.sources.clear(),
            "provider" => app.definition.provider.binding_id = "bnd_changed".parse().unwrap(),
            "key" => app.definition.secrets.agent_key_hash = format!("sha256:{}", "1".repeat(64)),
            "bot" => app.definition.secrets.telegram_bot_token = Some(Secret::new("88:fixture")),
            "policy" => {
                if let Source::Telegram { admission, .. } = &mut app.definition.sources[0] {
                    *admission = AdmissionPolicy::Allowlist {
                        sender_ids: vec!["99".into()],
                    };
                }
            }
            _ => unreachable!(),
        }
        app.update().await;
        assert!(
            !effect(&app, &reference, "delete", "revoked", json!({}))
                .send()
                .await
                .unwrap()
                .status()
                .is_success(),
            "{changed}"
        );
        assert_eq!(platform.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn telegram_media_named_consumer_and_expiry_do_not_allow_borrowed_or_edit_authority() {
    let (mut app, platform) = app().await;
    let reviewer = routing_fixture::REVIEWER;
    let key = routing_fixture::agent_key(reviewer);
    let mut definition = app.definition.to_blob_json();
    definition["routing"] = json!({"version":1,"default_consumer":{"name":"lead"},"consumers":[routing_fixture::consumer(reviewer,"reviewer","cma_codex")],"default_targets":["default"]});
    definition["secrets"]["consumer_keys"] = json!([[reviewer, key]]);
    app.definition = serde_json::from_value(definition).unwrap();
    app.update().await;
    app.restart().await;
    let event = app
        .event_for(
            "42",
            "reviewer-upload",
            Some(cmaeg_core::ids::ConsumerId::parse(reviewer).unwrap()),
            Secret::new(&key),
        )
        .await;
    let sent = upload(&app, &event, &key).await;
    let reference = MessageRef::new(sent["message_ref"].as_str().unwrap()).unwrap();
    assert_eq!(
        effect(&app, &reference, "delete", "borrowed", json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut payload = reference.open(&app.state.keys).unwrap().payload;
    let authority = payload.telegram_effect.as_mut().unwrap();
    authority.issued_at = support::now() - 86401;
    authority.expires_at = support::now() - 1;
    let expired = MessageRef::seal(&payload, &app.state.keys).unwrap();
    assert_eq!(
        effect_as(&app, &expired, "delete", "expired", json!({}), &key)
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    assert_eq!(
        receipt(
            effect_as(
                &app,
                &expired,
                "update",
                "edit",
                json!({"text":"updated"}),
                &key
            )
            .send()
            .await
            .unwrap()
        )
        .await["delivery"]["outcome"],
        "unsupported"
    );
    app.definition.routing = None;
    app.definition.secrets.consumer_keys.clear();
    app.update().await;
    assert_eq!(
        effect_as(&app, &reference, "delete", "removed", json!({}), &key)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(platform.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn telegram_media_different_keys_cannot_overlap_one_exact_message() {
    let (app, platform) = app().await;
    let reference = reference(&app).await;
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    *platform.gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let first = effect(
        &app,
        &reference,
        "reaction",
        "first",
        json!({"reaction":[]}),
    );
    let first = tokio::spawn(async move { first.send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(
        receipt(
            effect(&app, &reference, "delete", "second", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["reason"],
        "native_message_effect_busy"
    );
    release.add_permits(1);
    assert_eq!(
        receipt(first.await.unwrap()).await["delivery"]["outcome"],
        "confirmed"
    );
    assert_eq!(platform.calls.lock().unwrap().len(), 2);
}
