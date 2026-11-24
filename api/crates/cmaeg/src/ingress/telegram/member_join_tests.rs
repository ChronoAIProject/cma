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
