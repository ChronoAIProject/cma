use super::{http_tests::Server, *};
use cmaeg_core::seal::ReplyAddress;

async fn fixture(enabled: bool, opted_in: bool) -> Fixture {
    let mut f = Fixture::new(true).await;
    f.state.native_telegram_member_joins_enabled = enabled;
    let definition = &mut Arc::make_mut(&mut f.channel).definition;
    let Source::Telegram {
        admission,
        member_join_chat_ids,
        ..
    } = &mut definition.sources[0]
    else {
        panic!()
    };
    *admission = serde_json::from_value(json!({
        "type": "scoped", "senders": {"type": "open"},
        "chats": {"type": "allowlist", "ids": ["-100", "-101"]}, "groups": "all",
    }))
    .unwrap();
    if opted_in {
        member_join_chat_ids.push("-100".into());
    }
    definition.routing.as_mut().unwrap().default_targets =
        vec![crate::ingress::routing_plan::fixture::id("default")];
    definition.validate().unwrap();
    f.publish_channel();
    f
}

fn update(f: &Fixture, members: Value) -> Value {
    json!({
        "update_id": 405,
        "message": {
            "message_id": 10,
            "date": f.now - 1,
            "chat": {"id": -100, "type": "supergroup"},
            "from": {"id": 42, "is_bot": false, "first_name": "Inviter"},
            "new_chat_members": members,
        },
    })
}

#[tokio::test]
async fn telegram_member_joins_authenticate_group_one_turn_and_replay_exactly() {
    for multiple in [false, true] {
        let f = fixture(true, true).await;
        let mut members = vec![json!({
            "id": 61, "is_bot": false, "first_name": "Ada", "last_name": "Example",
        })];
        let mut expected =
            vec![json!({"id": "61", "display_name": "Ada Example", "is_bot": false})];
        if multiple {
            let duplicate = members[0].clone();
            members.extend([
                json!({"id": 77, "is_bot": true, "first_name": "Receiving bot"}),
                json!({"id": 62, "is_bot": false, "first_name": "Jo"}),
                duplicate,
            ]);
            expected.push(json!({"id": "62", "display_name": "Jo", "is_bot": false}));
        }
        let raw = update(&f, json!(members));
        let server = Server::new(&f).await;
        assert_eq!(server.send(&raw, "wrong-secret").await.0, 401);
        assert!(f.plan("telegram:77:405").await.is_none());

        let (status, body) = server.send(&raw, "fixture-webhook").await;
        assert_eq!(status, 202, "{body}");
        assert_eq!(body["status"], "accepted");
        let plan = f.plan("telegram:77:405").await.unwrap();
        assert_eq!(plan.activity.actor.kind, ActorKind::System);
        assert_eq!(plan.activity.actor.id, "telegram:system");
        assert_eq!(plan.activity.conversation.id, "-100");
        let ActivityKind::Event { ce, attachments } = &plan.activity.kind else {
            panic!("join update must produce an event");
        };
        assert!(attachments.is_empty());
        assert_eq!(ce.r#type, "telegram.group_members_joined");
        assert_eq!(ce.data, Some(json!({"members": expected})));
        assert_eq!(plan.targets.len(), 1);
        let event = &plan.targets[0].launch.event;
        assert!(event.telegram_admission.is_none());
        assert!(matches!(&event.reply, ReplyAddress::Telegram {
            bot_id, chat_id, message_id, thread_id: None,
        } if bot_id == "77" && chat_id == "-100" && message_id == "10"));

        let writes = f.server.memory.writes.load(Ordering::SeqCst);
        drop(server);
        let restored = Server::new(&f).await;
        assert_eq!(restored.send(&raw, "fixture-webhook").await, (status, body));
        assert_eq!(f.plan("telegram:77:405").await.unwrap(), plan);
        assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
        assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn telegram_member_joins_refuse_bot_only_disabled_and_unselected_chats() {
    for (enabled, opted_in, chat, member) in [
        (
            false,
            true,
            -100,
            json!({"id":61,"is_bot":false,"first_name":"Ada"}),
        ),
        (
            true,
            false,
            -100,
            json!({"id":61,"is_bot":false,"first_name":"Ada"}),
        ),
        (
            true,
            true,
            -101,
            json!({"id":61,"is_bot":false,"first_name":"Ada"}),
        ),
        (
            true,
            true,
            -100,
            json!({"id":77,"is_bot":true,"first_name":"Receiving bot"}),
        ),
        (
            true,
            true,
            -100,
            json!({"id":63,"is_bot":true,"first_name":"Another bot"}),
        ),
    ] {
        let f = fixture(enabled, opted_in).await;
        let mut raw = update(&f, json!([member]));
        raw["message"]["chat"]["id"] = chat.into();
        let server = Server::new(&f).await;
        let (status, body) = server.send(&raw, "fixture-webhook").await;
        assert_eq!(status, 202, "{body}");
        assert_eq!(body["reason"], "system_event");
        assert!(f.plan("telegram:77:405").await.is_none());
        assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn telegram_member_joins_request_retry_when_durable_routing_is_unavailable() {
    let mut f = fixture(true, true).await;
    f.state.routed = None;
    let raw = update(&f, json!([{"id":61,"is_bot":false,"first_name":"Ada"}]));
    let server = Server::new(&f).await;
    assert_eq!(server.send(&raw, "fixture-webhook").await.0, 503);
    assert!(f.plan("telegram:77:405").await.is_none());
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn telegram_relay_member_joins_preserve_the_reply_token_and_durable_update_identity() {
    use crate::ingress::relay::{self, facts::Observation, verify::VerifiedCallback};
    use cmaeg_core::channel::{Platform, Secret};

    let mut f = Fixture::new(false).await;
    let definition = &mut Arc::make_mut(&mut f.channel).definition;
    let route = "r".repeat(200);
    let mut source = crate::channels::routing_fixture::relay("telegram", &route);
    source["bot_id"] = "77".into();
    source["member_join_chat_ids"] = json!(["-100"]);
    source["admission"] = json!({
        "type":"scoped", "groups":"all", "senders":{"type":"open"},
        "chats":{"type":"allowlist","ids":["-100"]},
    });
    definition.sources = vec![serde_json::from_value(source).unwrap()];
    definition.routing.as_mut().unwrap().default_targets =
        vec![crate::ingress::routing_plan::fixture::id("default")];
    definition.validate().unwrap();
    f.publish_channel();

    let raw = update(&f, json!([{"id":61,"is_bot":false,"first_name":"Ada"}]));
    let body = json!({
        "message_id":"message-1","correlation_id":"correlation-1","platform":"telegram",
        "agent":{"api_key_id":"key-1","name":""},
        "conversation":{"id":route,"platform_id":"-100","type":"group"},
        "sender":{"platform_id":"42","display_name":"Inviter"},
        "content":{"type":"unknown"},"timestamp":"2026-10-26T00:00:00Z",
        "raw_platform_data":raw,
    });
    let request = |body: &Value| {
        let payload = serde_json::from_value(body.clone()).unwrap();
        let Observation::Message(facts) =
            relay::telegram_member_joins::observe(&f.channel.definition.sources[0], &payload)
                .unwrap()
                .unwrap()
        else {
            panic!("expected join event");
        };
        let verified = VerifiedCallback {
            payload,
            reply_token: Secret::new("fixture-reply-token"),
            reply_expires_at: f.now + 300,
            callback_deadline: f.now + 300,
        };
        let crate::ingress::media_guard::Built::Launch(request) = relay::activity::launch_request(
            f.channel.clone(),
            Platform::Telegram,
            verified,
            facts,
            &f.state.master_keys,
            Instant::now(),
            f.now,
        )
        .unwrap() else {
            panic!("expected relay launch");
        };
        *request
    };
    let mut second = body.clone();
    second["message_id"] = "message-2".into();
    second["raw_platform_data"]["update_id"] = 406.into();
    let mut recorded = Vec::new();
    for (body, original_update, original_message) in [
        (&body, "405", "message-1"),
        (&second, "406", "message-2"),
        (&body, "405", "message-1"),
    ] {
        let request = request(body);
        let event_id = request.activity.event_id.clone();
        assert_eq!(request.activity.actor.id, "telegram:system");
        assert_eq!(request.activity.actor.display_name, None);
        let ActivityKind::Event { ce, .. } = &request.activity.kind else {
            panic!("expected relay group join");
        };
        assert_eq!(ce.id, original_update);
        assert_eq!(ce.source, format!("nyxid:route:{route}"));
        let source_bytes = serde_json::to_vec(body).unwrap();
        let source = f.state.capture_routed(&request, &source_bytes, true);
        let hold = f.state.inflight.clone().try_acquire_owned().unwrap();
        assert!(matches!(
            f.state.launch_verified(request, source, hold).await,
            LaunchOutcome::Admitted { .. }
        ));
        let plan = f.plan(event_id.as_str()).await.unwrap();
        let target = &plan.targets[0].launch;
        assert!(target.consumer.is_default());
        assert!(matches!(&target.event.reply, ReplyAddress::NyxidRelay {
            route_id, message_id, reply_token, ..
        } if route_id == &route && message_id == original_message
            && reply_token.as_str() == "fixture-reply-token"));
        recorded.push(plan);
    }
    assert_ne!(recorded[0].identity.event_id, recorded[1].identity.event_id);
    assert_ne!(
        recorded[0].targets[0].launch.idempotency_key,
        recorded[1].targets[0].launch.idempotency_key,
    );
    assert_eq!(recorded[0], recorded[2]);
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}
