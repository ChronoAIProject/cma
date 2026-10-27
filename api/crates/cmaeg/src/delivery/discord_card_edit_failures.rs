use super::*;

#[tokio::test]
async fn discord_card_edit_wrong_receipts_are_durable_unknown_without_retry_or_new_authority() {
    for case in [
        "id",
        "channel",
        "author",
        "content",
        "components",
        "disabled",
        "lost",
    ] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        echo(&f, move |body| match case {
            "id" => body["id"] = "801".into(),
            "channel" => body["channel_id"] = "100".into(),
            "author" => body["author"]["id"] = "78".into(),
            "content" => body["content"] = "different".into(),
            "disabled" => body["components"][0]["components"][0]["disabled"] = true.into(),
            _ => body["components"] = json!([]),
        });
        if case == "lost" {
            // Preserve authenticated GET preflight, then lose the PATCH response.
            let memory = Arc::downgrade(&f.rest.memory);
            *f.rest.memory.on_call.lock().unwrap() = Some(Arc::new(move |call| {
                let memory = memory.upgrade().unwrap();
                if call.method == "GET" {
                    memory
                        .responses
                        .lock()
                        .unwrap()
                        .push_back((StatusCode::OK, json!({"id":"99","type":1})));
                } else {
                    *memory.broken.lock().unwrap() = true;
                }
            }));
        }
        let reference = reference(&f, &card);
        let sender = editing(&f);
        let receipt = update(&sender, &f, &reference, &message(true)).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            "{case}: {:?}",
            receipt.delivery
        );
        assert!(receipt.message_ref.is_none());
        assert!(f.store.load(&card.card.id).await.unwrap().0.superseded);
        let replay = update(&sender, &f, &reference, &message(true)).await;
        assert_eq!(
            serde_json::to_value(receipt).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
        assert!(matches!(
            update(&sender, &f, &reference, &message(false))
                .await
                .delivery,
            DeliveryOutcome::Unsupported
        ));
        assert_eq!(patches(&f), 1, "{case}");
    }
}

#[tokio::test]
async fn discord_card_edit_receipt_storage_loss_stays_fenced_across_restart() {
    for after_commit in [false, true] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        let record = card.card.id.clone();
        let backend = f.setup.fixture.memory.clone();
        echo(&f, move |_| {
            *backend.fault.lock().unwrap() = Some((record.clone(), after_commit));
        });
        let reference = reference(&f, &card);
        let result = update(&editing(&f), &f, &reference, &message(true)).await;
        assert!(matches!(result.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(result.message_ref.is_none());
        let mut restarted = sender(&f, false);
        restarted.relay = restarted.relay.with_interactions(Arc::new(
            crate::interactions::InteractionStore::new(
                f.setup.state.resolver.record_client(),
                f.keys(),
                f.provider.client(true, std::time::Duration::from_secs(5)),
            ),
        ));
        let replay = update(&restarted, &f, &reference, &message(true)).await;
        assert_eq!(
            matches!(replay.delivery, DeliveryOutcome::Confirmed { .. }),
            after_commit
        );
        assert_eq!(patches(&f), 1);
    }
}

#[tokio::test]
async fn discord_card_edits_recheck_source_provider_consumer_and_exact_thread() {
    for case in [
        "source",
        "provider",
        "consumer",
        "message",
        "thread_parent",
        "thread_guild",
        "thread_type",
        "bot",
    ] {
        let mut f = FixtureState::named_thread(11).await;
        let card = f.card(&message(false)).await;
        let mut reference = reference(&f, &card);
        match case {
            "message" => reference.platform_message_id = "801".into(),
            "consumer" => reference.consumer = None,
            "source" | "provider" => {
                let mut value = f.setup.channel.to_blob_json();
                if case == "provider" {
                    value["routing"]["consumers"][0]["provider"]["binding_id"] = "rebound".into();
                } else {
                    value["sources"][0]["admission"] =
                        json!({"type":"allowlist","sender_ids":["43"]});
                }
                f.setup.channel = serde_json::from_value(value).unwrap();
                f.setup.fixture.replace(&f.setup.channel);
                f.channel = f
                    .setup
                    .state
                    .resolver
                    .resolve_current(f.channel.id.as_str())
                    .await
                    .unwrap();
            }
            "bot" => *f.rest.memory.bot.lock().unwrap() = json!({"id":"78","bot":true}),
            _ => {
                let mut metadata = f.thread_metadata();
                metadata[match case {
                    "thread_parent" => "parent_id",
                    "thread_guild" => "guild_id",
                    _ => "type",
                }] = if case == "thread_type" {
                    10.into()
                } else {
                    "101".into()
                };
                f.rest
                    .memory
                    .responses
                    .lock()
                    .unwrap()
                    .push_back((StatusCode::OK, metadata));
            }
        }
        echo(&f, |_| {});
        let result = update(&editing(&f), &f, &reference, &message(true)).await;
        assert!(
            !matches!(result.delivery, DeliveryOutcome::Confirmed { .. }),
            "{case}"
        );
        assert_eq!(patches(&f), 0, "{case}");
        assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
    }
}

#[tokio::test]
async fn native_card_edit_shape_preflight_preserves_old_authority_without_platform_io() {
    for case in ["media", "reply", "thread", "oversize", "multiselect"] {
        let f = FixtureState::new().await;
        let card = f.card(&message(false)).await;
        let mut replacement = serde_json::to_value(message(true)).unwrap();
        match case {
            "media" => {
                replacement["native_attachments"] =
                    json!([{"kind":"file","source":{"type":"base64","data":"YQ=="}}])
            }
            "reply" => replacement["reply_to"] = "cmaeg1.mr.invalid".into(),
            "thread" => replacement["thread_reply"] = true.into(),
            "oversize" => replacement["text"] = "x".repeat(2001).into(),
            _ => replacement = serde_json::to_value(multiselect_message()).unwrap(),
        }
        let mut sender = editing(&f);
        sender.relay = sender.relay.with_discord_multiselect(false);
        let result = update(
            &sender,
            &f,
            &reference(&f, &card),
            &serde_json::from_value(replacement).unwrap(),
        )
        .await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Unsupported),
            "{case}: {:?}",
            result.delivery
        );
        assert!(f.rest.memory.calls.lock().unwrap().is_empty());
        assert!(!f.store.load(&card.card.id).await.unwrap().0.superseded);
    }
}
