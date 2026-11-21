use super::*;
use cmaeg_core::{activity::ConversationKind, channel::AdmissionPolicy};

pub(super) async fn channel_fixture() -> Fixture {
    let mut f = Fixture::new(false).await;
    let Source::Telegram { channel_posts, .. } =
        &mut Arc::make_mut(&mut f.channel).definition.sources[0]
    else {
        panic!()
    };
    *channel_posts = true;
    let mut message = f.raw.as_object_mut().unwrap().remove("message").unwrap();
    message["chat"] = json!({"id":-100,"type":"channel","title":"News"});
    message.as_object_mut().unwrap().remove("from");
    message["sender_chat"] = message["chat"].clone();
    f.raw["channel_post"] = message;
    f.publish_channel();
    f
}
pub(super) fn edit(f: &Fixture) -> Value {
    let mut message = f.raw["channel_post"].clone();
    message["edit_date"] = f.now.into();
    message["text"] = "/to nyx edited channel post".into();
    json!({"update_id":102,"edited_channel_post":message})
}
fn observed(
    raw: Value,
    media: bool,
    opted: bool,
) -> Result<Outcome, crate::ingress::relay::VerificationError> {
    crate::ingress::telegram::observe_source(
        raw,
        "77",
        Some("golden_bot"),
        media,
        &AdmissionPolicy::All {},
        opted,
    )
}

#[tokio::test]
async fn channel_identity_is_unknown_even_with_user_signature_and_edits_keep_original_routing() {
    let mut f = channel_fixture().await;
    f.raw["channel_post"]["from"] = json!({"id":42,"is_bot":false,"first_name":"not an authority"});
    f.raw["channel_post"]["author_signature"] = "signed publisher".into();
    let original = f.admit_original().await;
    assert_eq!(original.activity.actor.id, "telegram:chat:-100");
    assert_eq!(original.activity.actor.kind, ActorKind::Unknown);
    assert_eq!(
        original.activity.conversation.kind,
        ConversationKind::Channel
    );
    assert_eq!(original.activity.conversation.thread_id, None);
    let definition = &mut Arc::make_mut(&mut f.channel).definition;
    definition.partition.key = cmaeg_core::channel::PartitionKey::Fixed;
    definition.routing.as_mut().unwrap().default_targets =
        vec![crate::ingress::routing_plan::fixture::id(
            crate::channels::routing_fixture::NYX,
        )];
    assert!(matches!(
        f.launch(&edit(&f)).await,
        LaunchOutcome::Admitted { .. }
    ));
    let next = f.plan("telegram:77:102").await.unwrap();
    assert_eq!(next.activity.actor, original.activity.actor);
    assert_eq!(next.activity.conversation, original.activity.conversation);
    for (next, old) in next.targets.iter().zip(&original.targets) {
        assert_eq!(next.launch.consumer, old.launch.consumer);
        assert_eq!(next.launch.provider, old.launch.provider);
        assert_eq!(
            next.launch.event.partition_key,
            old.launch.event.partition_key
        );
        assert_eq!(
            next.launch.event.conversation_id,
            old.launch.event.conversation_id
        );
        assert!(
            matches!(&next.launch.event.reply,ReplyAddress::Telegram { chat_id,message_id,thread_id,.. }
            if chat_id=="-100" && message_id=="5" && thread_id.is_none())
        );
    }
    assert_eq!(next.targets.len(), original.targets.len());
    let ActivityKind::Event { ce, .. } = next.activity.kind else {
        panic!()
    };
    assert_eq!(ce.r#type, "cma.message.updated.v1");
    assert_eq!(
        ce.data.as_ref().unwrap()["original_author"]["kind"],
        "unknown"
    );
    let before = f.server.memory.writes.load(Ordering::SeqCst);
    assert!(matches!(
        f.launch(&edit(&f)).await,
        LaunchOutcome::Admitted { .. }
    ));
    assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), before);
    let mut altered_replay = edit(&f);
    altered_replay["edited_channel_post"]["text"] = "changed under the same update".into();
    assert!(
        matches!(f.launch(&altered_replay).await, LaunchOutcome::Refused(problem) if problem.status == 409)
    );
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn channel_media_edits_preserve_native_attachment_facts_and_original_mention_admission() {
    let mut f = channel_fixture().await;
    f.state.media_enabled = true;
    let Source::Telegram { admission, .. } =
        &mut Arc::make_mut(&mut f.channel).definition.sources[0]
    else {
        panic!()
    };
    *admission = serde_json::from_value(json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"allowlist","ids":["-100"]},"groups":"mention"})).unwrap();
    let message = &mut f.raw["channel_post"];
    message.as_object_mut().unwrap().remove("text");
    message["caption"] = "@golden_bot caption".into();
    message["caption_entities"] = json!([{"type":"mention","offset":0,"length":11}]);
    message["document"] = json!({"file_id":"original-file","file_unique_id":"original-unique","file_name":"note.txt","mime_type":"text/plain","file_size":29});
    message["media_group_id"] = "123456789".into();
    f.publish_channel();
    let original = f.admit_original().await;
    assert!(
        matches!(&original.activity.kind, ActivityKind::Message { album_id: Some(album), .. } if album == "123456789")
    );
    let mut changed = edit(&f);
    let message = &mut changed["edited_channel_post"];
    message.as_object_mut().unwrap().remove("text");
    message.as_object_mut().unwrap().remove("caption_entities");
    message["caption"] = "updated caption without mention".into();
    message["document"]["file_id"] = "edited-file".into();
    message["document"]["file_unique_id"] = "edited-unique".into();
    assert!(matches!(
        f.launch(&changed).await,
        LaunchOutcome::Admitted { .. }
    ));
    let next = f.plan("telegram:77:102").await.unwrap();
    let ActivityKind::Event { ce, .. } = next.activity.kind else {
        panic!()
    };
    let patch = &ce.data.as_ref().unwrap()["patch"];
    assert_eq!(patch["text"], "updated caption without mention");
    assert_eq!(patch["attachments"][0]["source"]["type"], "telegram");
    assert_eq!(patch["attachments"][0]["source"]["file_id"], "edited-file");
    assert_eq!(patch["attachments"][0]["filename"], "note.txt");
    f.state.media_enabled = false;
    assert!(f.verified(&changed).await.is_err());
}

#[tokio::test]
async fn channel_posts_seal_only_original_destination_without_human_effect_authority() {
    let f = channel_fixture().await;
    let Outcome::Admitted(post) = observed(f.raw.clone(), false, true).unwrap() else {
        panic!()
    };
    let crate::ingress::media_guard::Built::Launch(request) = crate::ingress::telegram::launch(
        f.channel.clone(),
        "77",
        *post,
        &f.state.master_keys,
        Instant::now(),
        f.now,
        true,
    )
    .unwrap() else {
        panic!()
    };
    let event = request
        .event_ref
        .open(&f.state.master_keys, f.now)
        .unwrap()
        .payload;
    assert!(event.telegram_admission.is_none());
    assert!(
        matches!(event.reply,ReplyAddress::Telegram { chat_id,message_id,thread_id,.. }
        if chat_id=="-100" && message_id=="5" && thread_id.is_none())
    );
}

#[tokio::test]
async fn channel_edits_require_retained_original_owner_credentials_and_unexpired_context() {
    let mut f = channel_fixture().await;
    f.admit_original().await;
    let channel = f.channel.clone();
    for field in ["owner", "bot_token", "webhook_secret", "admission"] {
        let definition = &mut Arc::make_mut(&mut f.channel).definition;
        match field {
            "owner" => definition.owner.subject = "other-owner".into(),
            "bot_token" => {
                definition.secrets.telegram_bot_token =
                    Some(cmaeg_core::channel::Secret::new("77:rotated"))
            }
            "webhook_secret" => {
                definition.secrets.telegram_webhook_secret =
                    Some(cmaeg_core::channel::Secret::new("rotated"))
            }
            _ => {
                let Source::Telegram { admission, .. } = &mut definition.sources[0] else {
                    panic!()
                };
                *admission =
                    serde_json::from_value(json!({"type":"allowlist","sender_ids":["42"]}))
                        .unwrap();
            }
        }
        assert!(
            matches!(f.verified(&edit(&f)).await, Err(Error::Context)),
            "{field}"
        );
        f.channel = channel.clone();
    }
    let raw = edit(&f);
    assert!(matches!(
        prepare(
            &f.state,
            f.channel.clone(),
            "77",
            &raw,
            &serde_json::to_vec(&raw).unwrap(),
            Instant::now(),
            f.now + 7 * 86400
        )
        .await,
        Err(Error::Context)
    ));
}

#[tokio::test]
async fn channel_edits_refuse_other_identity_type_topic_date_and_revoked_source() {
    let mut f = channel_fixture().await;
    assert!(matches!(f.verified(&edit(&f)).await, Err(Error::Context)));
    f.admit_original().await;
    for (field, value) in [
        ("sender_chat", json!({"id":-101,"type":"channel"})),
        ("chat", json!({"id":-100,"type":"supergroup"})),
        ("message_thread_id", json!(12)),
        ("direct_messages_topic", json!({"topic_id":12})),
        ("date", json!(f.now - 20)),
        ("edit_date", json!(f.now - 20)),
    ] {
        let mut raw = edit(&f);
        raw["edited_channel_post"][field] = value;
        assert!(f.verified(&raw).await.is_err(), "{field}");
    }
    let mut wrong_kind = edit(&f);
    let message = wrong_kind
        .as_object_mut()
        .unwrap()
        .remove("edited_channel_post")
        .unwrap();
    wrong_kind["edited_message"] = message;
    assert!(f.verified(&wrong_kind).await.is_err());
    let Source::Telegram { channel_posts, .. } =
        &mut Arc::make_mut(&mut f.channel).definition.sources[0]
    else {
        panic!()
    };
    *channel_posts = false;
    assert!(matches!(f.verified(&edit(&f)).await, Err(Error::Context)));
}

#[tokio::test]
async fn channel_posts_use_native_media_gate_and_reject_unsupported_topic_or_forward_shapes() {
    let f = channel_fixture().await;
    assert!(matches!(
        observed(f.raw.clone(), false, false).unwrap(),
        Outcome::Refused(Refusal::SystemSender)
    ));
    let mut without_sender = f.raw.clone();
    without_sender["channel_post"]
        .as_object_mut()
        .unwrap()
        .remove("sender_chat");
    assert!(matches!(
        observed(without_sender, false, true).unwrap(),
        Outcome::Admitted(_)
    ));
    let mut media = f.raw.clone();
    media["channel_post"]
        .as_object_mut()
        .unwrap()
        .remove("text");
    media["channel_post"]["caption"] = "Caption".into();
    media["channel_post"]["document"] = json!({"file_id":"file-id","file_unique_id":"unique-id","file_name":"note.txt","mime_type":"text/plain","file_size":29});
    media["channel_post"]["media_group_id"] = "123456789".into();
    assert!(matches!(
        observed(media.clone(), false, true).unwrap(),
        Outcome::Refused(Refusal::AttachmentUnsupported)
    ));
    let Outcome::Admitted(post) = observed(media, true, true).unwrap() else {
        panic!()
    };
    assert_eq!(post.payload.content.text.as_deref(), Some("Caption"));
    assert_eq!(post.facts.attachments.len(), 1);
    assert_eq!(post.facts.album_id.as_deref(), Some("123456789"));
    assert!(matches!(
        post.facts.attachments[0].source,
        Some(cmaeg_core::activity::AttachmentSource::Telegram { .. })
    ));
    for (key, value) in [
        ("message_thread_id", json!(1)),
        ("is_topic_message", json!(true)),
        ("direct_messages_topic", json!({"topic_id":1})),
        ("business_connection_id", json!("business")),
        ("is_automatic_forward", json!(true)),
        ("edit_date", json!(f.now)),
    ] {
        let mut raw = f.raw.clone();
        raw["channel_post"][key] = value;
        assert!(
            matches!(observed(raw, true, true).unwrap(), Outcome::Refused(_)),
            "{key}"
        );
    }
    let mut mixed = f.raw.clone();
    mixed["message"] = mixed["channel_post"].clone();
    assert!(observed(mixed, true, true).is_err());
    for chat in [
        json!({"id":100,"type":"channel"}),
        json!({"id":-100,"type":"supergroup"}),
        json!({"id":-100,"type":"channel","is_direct_messages":true}),
    ] {
        let mut raw = f.raw.clone();
        raw["channel_post"]["chat"] = chat;
        assert!(observed(raw, true, true).is_err());
    }
    let mut raw = f.raw.clone();
    raw["channel_post"]["sender_chat"]["id"] = json!(-101);
    assert!(observed(raw, true, true).is_err());
    let mut raw = f.raw.clone();
    let mut group = raw.as_object_mut().unwrap().remove("channel_post").unwrap();
    group["chat"] = json!({"id":-200,"type":"supergroup"});
    group["from"] = json!({"id":42,"is_bot":false});
    raw["message"] = group;
    assert!(matches!(
        observed(raw, true, true).unwrap(),
        Outcome::Refused(_)
    ));
}

#[tokio::test]
async fn channel_admission_never_treats_channel_id_or_from_as_a_human_allowlist_match() {
    let f = channel_fixture().await;
    let facts = AdmissionFacts {
        receiving_bot_id: Some("77"),
        mentioned: Some(false),
        sender_id: "telegram:chat:-100",
        chat_id: "-100",
        is_group: true,
        is_bot_sender: false,
        is_system: false,
        addressed: Some(false),
    };
    for policy in [
        json!({"type":"allowlist","sender_ids":["42","telegram:chat:-100"]}),
        json!({"type":"scoped","senders":{"type":"allowlist","ids":["telegram:chat:-100"]},"chats":{"type":"open"},"groups":"all"}),
    ] {
        let policy: AdmissionPolicy = serde_json::from_value(policy).unwrap();
        assert_eq!(
            crate::ingress::telegram::channel_posts::admit_channel(&policy, &facts),
            Err(Refusal::UnknownSender)
        );
    }
    for (chats, groups, reason) in [
        (
            json!({"type":"allowlist","ids":["-101"]}),
            "all",
            Refusal::ChatDenied,
        ),
        (json!({"type":"open"}), "deny", Refusal::GroupDenied),
        (
            json!({"type":"open"}),
            "mention",
            Refusal::GroupNotAddressed,
        ),
    ] {
        let policy = serde_json::from_value(
            json!({"type":"scoped","senders":{"type":"open"},"chats":chats,"groups":groups}),
        )
        .unwrap();
        assert_eq!(
            crate::ingress::telegram::channel_posts::admit_channel(&policy, &facts),
            Err(reason)
        );
    }
    assert!(observed(f.raw, false, true).is_ok());
}
