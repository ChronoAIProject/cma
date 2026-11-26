use super::*;
use crate::ingress::launch::LaunchOutcome;
use std::sync::atomic::Ordering;
#[path = "events_fixture.rs"]
mod fixture;
use fixture::Fixture;
#[path = "anonymous_reaction_tests.rs"]
mod anonymous_tests;
#[path = "channel_post_tests.rs"]
mod channel_post_tests;
#[path = "events_http_tests.rs"]
mod http_tests;
#[path = "member_join_tests.rs"]
mod member_join_tests;

#[tokio::test]
async fn telegram_message_events_keep_original_agents_partition_and_thread_across_changed_text_and_policy(
) {
    for group in [false, true] {
        let mut f = Fixture::new(group).await;
        let original = f.admit_original().await;
        Arc::make_mut(&mut f.channel)
            .definition
            .routing
            .as_mut()
            .unwrap()
            .default_targets = vec![crate::ingress::routing_plan::fixture::id(
            crate::channels::routing_fixture::NYX,
        )];
        Arc::make_mut(&mut f.channel).definition.partition.key =
            cmaeg_core::channel::PartitionKey::Fixed;
        for raw in [f.edit(), f.reaction()] {
            assert!(matches!(
                f.launch(&raw).await,
                LaunchOutcome::Admitted { .. }
            ));
            let plan = f
                .plan(&format!("telegram:77:{}", raw["update_id"]))
                .await
                .unwrap();
            assert_eq!(plan.targets.len(), original.targets.len());
            for (next, old) in plan.targets.iter().zip(&original.targets) {
                assert_eq!(next.launch.consumer, old.launch.consumer);
                assert_eq!(next.launch.provider, old.launch.provider);
                assert_eq!(
                    next.launch.event.conversation_id,
                    old.launch.event.conversation_id
                );
                assert_eq!(
                    next.launch.event.partition_key,
                    old.launch.event.partition_key
                );
            }
            assert_eq!(plan.activity.conversation, original.activity.conversation);
            let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
                panic!()
            };
            assert_eq!(ce.subject.as_deref(), Some("5"));
            assert_eq!(
                ce.data.as_ref().unwrap()["original_event_id"],
                "telegram:77:101"
            );
            if raw.get("message_reaction").is_some() {
                assert_eq!(plan.activity.actor.id, "43");
                assert_eq!(ce.data.as_ref().unwrap()["original_author"]["id"], "42");
                assert_eq!(ce.data.as_ref().unwrap()["new_reactions"], json!([]));
            }
            let before = f.server.memory.writes.load(Ordering::SeqCst);
            assert!(matches!(
                f.launch(&raw).await,
                LaunchOutcome::Admitted { .. }
            ));
            assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), before);
        }
        assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn telegram_message_event_replay_conflicts_and_revoked_source_or_provider_never_reroutes() {
    let mut f = Fixture::new(false).await;
    f.admit_original().await;
    let edit = f.edit();
    assert!(matches!(
        f.launch(&edit).await,
        LaunchOutcome::Admitted { .. }
    ));
    let mut changed = edit.clone();
    changed["edited_message"]["text"] = "altered replay".into();
    assert!(
        matches!(f.launch(&changed).await,LaunchOutcome::Refused(problem) if problem.status==409)
    );
    Arc::make_mut(&mut f.channel)
        .definition
        .routing
        .as_mut()
        .unwrap()
        .consumers[0]
        .provider
        .binding_id = "changed".parse().unwrap();
    assert!(matches!(
        f.launch(&f.reaction()).await,
        LaunchOutcome::Refused(_)
    ));
    Arc::make_mut(&mut f.channel)
        .definition
        .secrets
        .telegram_bot_token = Some(cmaeg_core::channel::Secret::new("77:rotated"));
    assert!(matches!(f.verified(&edit).await, Err(Error::Context)));
}
#[tokio::test]
async fn telegram_message_events_require_exact_target_author_thread_and_attributable_reactor() {
    let f = Fixture::new(true).await;
    f.admit_original().await;
    for (pointer, value) in [
        ("/edited_message/from/id", json!(99)),
        ("/edited_message/chat/id", json!(-101)),
        ("/edited_message/message_id", json!(6)),
        ("/edited_message/message_thread_id", json!(124)),
        ("/edited_message/date", json!(f.now - 10)),
        ("/edited_message/edit_date", json!(f.now - 100)),
    ] {
        let mut raw = f.edit();
        *raw.pointer_mut(pointer).unwrap() = value;
        assert!(f.verified(&raw).await.is_err(), "{pointer}");
    }
    for (field, value) in [
        ("actor_chat", json!({"id":-100,"type":"supergroup"})),
        ("user", json!({"id":43,"is_bot":true})),
        (
            "new_reaction",
            json!([{"type":"emoji","emoji":"👍"},{"type":"emoji","emoji":"👍"}]),
        ),
        ("new_reaction", json!([{"type":"future_reaction"}])),
    ] {
        let mut raw = f.reaction();
        raw["message_reaction"][field] = value;
        assert!(f.verified(&raw).await.is_err(), "{field}");
    }
    let mut mixed = f.edit();
    mixed["message"] = f.raw["message"].clone();
    assert!(f.verified(&mixed).await.is_err());
    assert!(!requested(
        &json!({"update_id":1,"deleted_messages":{"message_ids":[5]}})
    ));
}
#[tokio::test]
async fn telegram_message_context_recovers_lost_write_ack_and_original_plan_is_required() {
    let f = Fixture::new(false).await;
    assert!(matches!(f.verified(&f.edit()).await, Err(Error::Context)));
    let request = f.request();
    let context = Context::capture(&f.state, &request, f.now).unwrap();
    let id = crate::connections::store::index_id(
        "telegram-message-context-v1",
        &[f.channel.definition.record_id.as_str(), "77", "42", "5"],
    );
    f.server.fault(id, true, false);
    assert!(context.remember(&f.state, &f.channel).await.is_err());
    // A locator alone cannot manufacture authority before the original commits.
    assert!(matches!(
        f.verified(&f.edit()).await,
        Err(Error::Unavailable)
    ));
    f.admit_original().await;
    assert!(matches!(
        f.launch(&f.edit()).await,
        LaunchOutcome::Admitted { .. }
    ));
}
