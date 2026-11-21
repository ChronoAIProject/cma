use super::*;

#[tokio::test]
async fn native_lark_forms_retain_an_explicit_empty_optional_submission() {
    let cards = Cards::new("feishu").await;
    let event = cards
        .app
        .event_ref
        .open(&cards.app.state.keys, now())
        .unwrap()
        .payload;
    let mut message = form();
    if let cmaeg_core::message::Block::Form { fields, .. } = &mut message.blocks[0] {
        fields.iter_mut().for_each(|field| field.required = false);
    }
    let mut card = cards
        .store
        .prepare_lark_form(cards.channel().await.as_ref(), &event, &message)
        .await
        .unwrap();
    cards
        .store
        .bind(
            &mut card,
            Destination {
                outbound_id: "outbound-1".into(),
                platform_id: "om_card".into(),
            },
        )
        .await
        .unwrap();
    let accepted = cards.send(&callback(&cards, &card, json!({}))).await;
    assert_eq!(accepted.1["status"], "accepted");
    assert_eq!(
        cards
            .send(&callback(&cards, &card, json!({"f0":"","f1":null})))
            .await,
        accepted
    );
    let saved = serde_json::to_value(cards.store.load(&card.card.id).await.unwrap().0).unwrap();
    assert_eq!(saved["consumed"]["submitted"], json!({}));
    let requests = cards.app.fixture.recorder.requests.lock().unwrap();
    assert_eq!(requests[0].activity, requests[1].activity);
}

#[tokio::test]
async fn native_lark_forms_deliver_typed_fields_to_the_original_provider() {
    for (platform, named) in [("lark", false), ("feishu", true)] {
        let cards = Cards::new(platform).await;
        let (card, original) = prepare(&cards, named, true).await;
        assert_eq!(
            cards.send(&callback(&cards, &card, values())).await.1["status"],
            "accepted"
        );
        let request = cards
            .app
            .fixture
            .recorder
            .requests
            .lock()
            .unwrap()
            .pop()
            .unwrap();
        let (sender, mut commands) = tokio::sync::mpsc::channel(32);
        let runner = TurnRunner::new(
            cards.app.state.providers.clone(),
            cards.app.state.tracker.clone(),
            Arc::new(cmaeg::delivery::router::ShardedScheduler::new(sender)),
            Default::default(),
        );
        let before = cards.app.fake.log().len();
        assert!(matches!(
            runner.launch(request).await,
            LaunchOutcome::Accepted { .. }
        ));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(command) = commands.recv().await {
                if matches!(
                    command,
                    cmaeg::delivery::intents::DeliveryCommand::StreamOutcome { .. }
                ) {
                    return;
                }
            }
            panic!("provider turn did not finish");
        })
        .await
        .unwrap();
        let log = cards.app.fake.log();
        let turns: Vec<_> = log[before..]
            .iter()
            .filter(|call| call.method == "POST" && call.path == "/api/v2/responses")
            .collect();
        assert_eq!(turns.len(), 1);
        let body: Value = serde_json::from_str(&turns[0].body).unwrap();
        assert_eq!(body["conversation"], original.conversation_id.as_str());
        assert_eq!(
            body["event_context"]["activity"]["kind"]["values"]["fields"],
            json!({"reason":"Ready","amount":12.5,"day":"2024-02-29","scope":"private-write"})
        );
        if named {
            assert_eq!(
                turns[0].authorization,
                Some(format!("Bearer {}", reviewer_key()))
            );
        }
    }
}

#[tokio::test]
async fn native_lark_forms_replacement_fences_original_revision_and_never_upgrades_plain_cards() {
    let mut cards = Cards::new("lark").await;
    let (card, _) = prepare(&cards, true, true).await;
    let click = callback(&cards, &card, values());
    assert_eq!(cards.send(&click).await.1["status"], "accepted");
    let reference = card
        .card
        .reference
        .as_ref()
        .unwrap()
        .open(&cards.app.state.keys)
        .unwrap()
        .payload;
    let mut next = cards
        .store
        .replacement_lark_form(cards.channel().await.as_ref(), &reference, &form())
        .await
        .unwrap();
    assert_eq!(next.card.revision, 2);
    cards
        .store
        .bind(
            &mut next,
            Destination {
                outbound_id: "outbound-1".into(),
                platform_id: "om_card".into(),
            },
        )
        .await
        .unwrap();
    cards.restart();
    assert_eq!(
        cards.send(&click).await.1["reason"],
        "interaction_expired_or_invalid"
    );
    let mut fresh = callback(&cards, &next, values());
    fresh.payload["raw_platform_data"]["header"]["event_id"] = "next-form-click".into();
    assert_eq!(cards.send(&fresh).await.1["status"], "accepted");
    let saved = serde_json::to_value(cards.store.load(&next.card.id).await.unwrap().0).unwrap();
    assert_eq!(saved["consumed"]["submitted"]["scope"], "private-write");

    let cards = Cards::new("feishu").await;
    let plain = cards.card(false).await;
    assert_eq!(
        cards.send(&cards.click(&plain)).await.1["status"],
        "accepted"
    );
    let reference = plain
        .card
        .reference
        .as_ref()
        .unwrap()
        .open(&cards.app.state.keys)
        .unwrap()
        .payload;
    assert!(cards
        .store
        .replacement_lark_form(cards.channel().await.as_ref(), &reference, &form())
        .await
        .is_err());
    assert!(!cards.store.load(&plain.card.id).await.unwrap().0.superseded);
}

#[tokio::test]
async fn native_lark_forms_competing_submissions_preserve_one_winning_result() {
    let mut cards = Cards::new("lark").await;
    let (card, _) = prepare(&cards, false, true).await;
    cards.restart();
    let first = callback(&cards, &card, json!({"f0":"First","f3":"0"}));
    let mut second = callback(&cards, &card, json!({"f0":"Second","f3":"1"}));
    second.payload["raw_platform_data"]["header"]["event_id"] = "competing-form-click".into();
    let results = tokio::join!(cards.send(&first), cards.send(&second));
    let results = [results.0, results.1];
    assert_eq!(
        results
            .iter()
            .filter(|(_, body)| body["status"] == "accepted")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|(_, body)| body["reason"] == "interaction_expired_or_invalid")
            .count(),
        1
    );
    assert_eq!(cards.app.fixture.recorder.requests.lock().unwrap().len(), 1);
    let saved = serde_json::to_value(cards.store.load(&card.card.id).await.unwrap().0).unwrap();
    let expected = if saved["consumed"]["callback_id"] == "form-click" {
        "First"
    } else {
        "Second"
    };
    assert_eq!(saved["consumed"]["submitted"]["reason"], expected);
}
