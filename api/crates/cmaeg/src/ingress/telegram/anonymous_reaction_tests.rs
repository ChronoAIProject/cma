use super::*;
use cmaeg_core::channel::{AdmissionPolicy, IdentityAdmission};

pub(super) fn aggregate(f: &Fixture) -> Value {
    json!({"update_id":301,"message_reaction_count":{
        "chat":f.raw["message"]["chat"],"message_id":5,"date":f.now,
        "reactions":[{"type":{"type":"emoji","emoji":"👍"},"total_count":4},
            {"type":{"type":"paid"},"total_count":1}]}})
}
fn anonymous(f: &Fixture) -> Value {
    let mut raw = f.reaction();
    raw["update_id"] = 302.into();
    raw["message_reaction"]
        .as_object_mut()
        .unwrap()
        .remove("user");
    raw["message_reaction"]["actor_chat"] = json!({"id":-200,"type":"channel","title":"Team"});
    raw
}

#[tokio::test]
async fn anonymous_reactions_preserve_original_targets_partition_topic_and_unknown_actor() {
    let mut f = Fixture::new(true).await;
    let original = f.admit_original().await;
    Arc::make_mut(&mut f.channel)
        .definition
        .routing
        .as_mut()
        .unwrap()
        .default_targets = vec![crate::ingress::routing_plan::fixture::id(
        crate::channels::routing_fixture::NYX,
    )];
    for raw in [aggregate(&f), anonymous(&f)] {
        assert!(matches!(
            f.launch(&raw).await,
            LaunchOutcome::Admitted { .. }
        ));
        let plan = f
            .plan(&format!("telegram:77:{}", raw["update_id"]))
            .await
            .unwrap();
        assert_eq!(plan.activity.actor.kind, ActorKind::Unknown);
        assert_ne!(plan.activity.actor.id, original.activity.actor.id);
        assert_eq!(plan.activity.conversation, original.activity.conversation);
        assert_eq!(plan.targets.len(), original.targets.len());
        for (actual, expected) in plan.targets.iter().zip(&original.targets) {
            assert_eq!(actual.launch.consumer, expected.launch.consumer);
            assert_eq!(actual.launch.provider, expected.launch.provider);
            assert_eq!(
                actual.launch.event.partition_key,
                expected.launch.event.partition_key
            );
            assert_eq!(
                actual.launch.event.conversation_id,
                expected.launch.event.conversation_id
            );
        }
        let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            panic!()
        };
        let data = ce.data.as_ref().unwrap();
        assert_eq!(data["original_author"]["id"], "42");
        if raw.get("message_reaction_count").is_some() {
            assert_eq!(data["observation"], "aggregate");
            assert_eq!(data["reaction_counts"].as_array().unwrap().len(), 2);
            assert!(data.get("old_reactions").is_none());
            assert!(data.get("new_reactions").is_none());
            assert_eq!(plan.activity.actor.id, "telegram:anonymous:-100");
        } else {
            assert_eq!(data["actor_chat"]["id"], -200);
            assert_eq!(plan.activity.actor.id, "telegram:chat:-200");
        }
        let writes = f.server.memory.writes.load(Ordering::SeqCst);
        assert!(matches!(
            f.launch(&raw).await,
            LaunchOutcome::Admitted { .. }
        ));
        assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
    }
    assert_eq!(f.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn anonymous_reactions_cannot_claim_allowlisted_humans_or_escape_original_source() {
    let mut f = Fixture::new(true).await;
    if let Source::Telegram { admission, .. } =
        &mut Arc::make_mut(&mut f.channel).definition.sources[0]
    {
        *admission = AdmissionPolicy::Allowlist {
            sender_ids: vec!["42".into()],
        };
    } else {
        panic!()
    }
    f.publish_channel();
    f.admit_original().await;
    for raw in [aggregate(&f), anonymous(&f)] {
        assert!(matches!(
            f.verified(&raw).await,
            Err(Error::Refused(Refusal::UnknownSender))
        ));
    }
    let f = Fixture::new(true).await;
    assert!(matches!(
        f.verified(&aggregate(&f)).await,
        Err(Error::Context)
    ));
    f.admit_original().await;
    let raw = aggregate(&f);
    assert!(matches!(
        f.launch(&raw).await,
        LaunchOutcome::Admitted { .. }
    ));
    let mut changed = raw.clone();
    changed["message_reaction_count"]["reactions"][0]["total_count"] = 9.into();
    assert!(
        matches!(f.launch(&changed).await,LaunchOutcome::Refused(problem) if problem.status==409)
    );
}

#[tokio::test]
async fn anonymous_reactions_reject_mixed_actors_duplicate_counts_and_unbounded_values() {
    let f = Fixture::new(true).await;
    f.admit_original().await;
    let mut both = anonymous(&f);
    both["message_reaction"]["user"] = json!({"id":43,"is_bot":false});
    assert!(f.verified(&both).await.is_err());
    for field in ["user", "actor_chat"] {
        let mut raw = aggregate(&f);
        raw["message_reaction_count"][field] = json!({"id":43});
        assert!(f.verified(&raw).await.is_err());
    }
    for counts in [
        json!([{"type":{"type":"emoji","emoji":"👍"},"total_count":-1}]),
        json!([{"type":{"type":"emoji","emoji":"👍"},"total_count":2147483648u64}]),
        json!([{"type":{"type":"future"},"total_count":1}]),
        json!([{"type":{"type":"paid"},"total_count":1},{"type":{"type":"paid"},"total_count":2}]),
        json!(vec![json!({"type":{"type":"paid"},"total_count":1}); 65]),
    ] {
        let mut raw = aggregate(&f);
        raw["message_reaction_count"]["reactions"] = counts;
        assert!(f.verified(&raw).await.is_err());
    }
    let mut empty = aggregate(&f);
    empty["message_reaction_count"]["reactions"] = json!([]);
    assert!(f.verified(&empty).await.is_ok());
    let policy = AdmissionPolicy::Scoped {
        senders: IdentityAdmission::Deny,
        chats: IdentityAdmission::Open,
        groups: cmaeg_core::channel::GroupAdmission::All,
        bot_messages: Default::default(),
    };
    assert!(reaction_facts::normalize(
        "message_reaction_count",
        &aggregate(&f)["message_reaction_count"],
        "-100",
        &policy
    )
    .is_err());
}
