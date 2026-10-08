use super::*;
use crate::delivery::event::key;
use cmaeg_core::{
    channel::ChannelDefinition,
    ids::ChannelId,
    seal::{ChannelPointer, Lineage, ReplyAddress},
};
use serde_json::{json, Value};

const REVIEWER: &str = "csm_00000000000a";

fn channel(edit: impl FnOnce(&mut Value)) -> Arc<ResolvedChannel> {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    edit(&mut value);
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    Arc::new(ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.group-fixture").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: std::time::Instant::now(),
        version: 1,
    })
}

/// A channel routing to `default` and `reviewer`, whose reply policy is `reply`.
fn routed(reply: Value) -> Arc<ResolvedChannel> {
    channel(|value| {
        let profile = value["profile"].clone();
        value["routing"] = json!({"version": 1, "default_consumer": {"name": "lead"},
            "consumers": [{"id": REVIEWER, "name": "reviewer", "profile": profile,
                "profile_digest": value["profile_digest"], "provider": value["provider"],
                "agent_key_hash": value["secrets"]["agent_key_hash"], "reply": reply,
                "created_at": "2026-10-01T00:00:00Z"}],
            "default_targets": ["default", REVIEWER]});
    })
}

fn reviewer() -> ConsumerId {
    ConsumerId::parse(REVIEWER).unwrap()
}

fn event(
    channel: &ResolvedChannel,
    event_id: &str,
    consumer: Option<ConsumerId>,
) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: event_id.parse().unwrap(),
        partition_key: event_id.into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::None,
        consumer,
        lineage: None,
    }
}

fn id(event: &EventRefPayload) -> EventKey {
    key(event.channel_id.as_str(), event.event_id.as_str())
}

#[test]
fn an_unrouted_group_holds_exactly_the_default_consumer() {
    let channel = channel(|value| value["reply"]["auto_final"] = false.into());
    let event = event(&channel, "msg-1", None);
    let mut group = EventGroup::new(channel.clone(), event.clone());
    assert_eq!(group.consumers.len(), 1);
    assert!(group.consumers[0].consumer.is_default());
    assert!(!group.consumers[0].policy.auto_final);
    assert!(!group.handoff);
    assert!(
        group.terminal_at.is_some(),
        "a group that never started expires"
    );
    // Joining again with the same, implicit, consumer finds the same entry.
    let mut explicit = event.clone();
    explicit.consumer = None;
    assert_eq!(group.consumer_mut(&explicit), 0);
    assert_eq!(group.consumers.len(), 1);
}

#[test]
fn consumers_join_by_id_and_the_group_is_terminal_only_when_all_are() {
    let channel = routed(json!({}));
    let mut group = EventGroup::new(channel.clone(), event(&channel, "msg-1", None));
    let at = group.consumer_mut(&event(&channel, "msg-1", Some(reviewer())));
    assert_eq!((at, group.consumers.len()), (1, 2));
    group.terminal_at = None;
    group.ack_due = Some(Instant::now());
    group.finish(0);
    assert!(!group.terminal());
    assert!(group.terminal_at.is_none(), "one consumer is still running");
    assert!(
        group.ack_due.is_some(),
        "the event's ack belongs to the group"
    );
    group.finish(at);
    assert!(group.terminal());
    assert!(group.terminal_at.is_some());
    assert!(group.ack_due.is_none());
}

#[test]
fn each_consumer_reads_its_own_policy_from_the_current_snapshot() {
    let first = routed(json!({"auto_final": false}));
    let mut group = EventGroup::new(first.clone(), event(&first, "msg-1", None));
    let at = group.consumer_mut(&event(&first, "msg-1", Some(reviewer())));
    assert!(
        group.consumers[0].policy.auto_final,
        "default keeps the channel's"
    );
    assert!(
        !group.consumers[at].policy.auto_final,
        "reviewer overrides it"
    );
    group.set_channel(routed(json!({"destination": "default"})));
    assert!(group.consumers[at].policy.auto_final);
    assert_eq!(group.consumers[at].policy.destination, ReplyChoice::Default);
    // A snapshot that no longer defines the reviewer keeps its last policy.
    group.set_channel(channel(|_| {}));
    assert_eq!(group.consumers[at].policy.destination, ReplyChoice::Default);
    // A consumer first seen after its removal answers at the origin under the channel's policy.
    let mut late = EventGroup::new(
        group.channel.clone(),
        event(&first, "msg-2", Some(reviewer())),
    );
    assert_eq!(late.consumers[0].policy.destination, ReplyChoice::Origin);
    assert!(late.consumers[0].policy.auto_final);
    assert_eq!(
        late.consumer_mut(&event(&first, "msg-2", Some(reviewer()))),
        0
    );
}

#[test]
fn a_handoff_group_is_marked_by_its_lineage() {
    let channel = routed(json!({}));
    let mut handoff = event(&channel, "msg-1", Some(reviewer()));
    handoff.lineage = Some(Lineage {
        root: "0123456789abcdef0123456789abcdef".into(),
        hop: 1,
        path: vec![reviewer()],
        reply: ReplyChoice::Origin,
    });
    assert!(EventGroup::new(channel, handoff).handoff);
}

/// Fill a worker with `MAX_EVENTS` live entries: two consumers in every
/// group but the last two, which hold one each.
fn full(channel: &Arc<ResolvedChannel>) -> (Groups, EventRefPayload, EventRefPayload) {
    let mut groups = Groups::default();
    let mut single = Vec::new();
    for index in 0..(MAX_EVENTS / 2 + 1) {
        let default = event(channel, &format!("msg-{index}"), None);
        let (group, _) = groups.join(id(&default), channel, &default);
        group.terminal_at = None;
        if index < MAX_EVENTS / 2 - 1 {
            let other = event(channel, &format!("msg-{index}"), Some(reviewer()));
            groups.join(id(&other), channel, &other);
        } else {
            single.push(default);
        }
    }
    assert_eq!(groups.entries(), MAX_EVENTS);
    let [held, _] = <[EventRefPayload; 2]>::try_from(single).unwrap();
    let newcomer = event(channel, &held.event_id.to_string(), Some(reviewer()));
    (groups, held, newcomer)
}

#[tokio::test(start_paused = true)]
async fn the_cap_counts_consumer_entries_and_expiry_releases_them() {
    let channel = routed(json!({}));
    let (mut groups, held, newcomer) = full(&channel);
    let fresh = event(&channel, "msg-new", None);
    assert!(
        groups.reserve(&id(&held), &consumer_of(&held)),
        "a present consumer proceeds"
    );
    assert!(
        !groups.reserve(&id(&newcomer), &consumer_of(&newcomer)),
        "a new consumer of an existing group is refused"
    );
    assert!(
        !groups.reserve(&id(&fresh), &consumer_of(&fresh)),
        "a new event is refused"
    );
    for group in groups.values_mut() {
        for at in 0..group.consumers.len() {
            group.finish(at);
        }
    }
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(
        !groups.reserve(&id(&fresh), &consumer_of(&fresh)),
        "still retained"
    );
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(groups.reserve(&id(&fresh), &consumer_of(&fresh)));
    assert_eq!(groups.entries(), 0, "expiry released every entry");
}

#[tokio::test(start_paused = true)]
async fn a_group_expires_30_seconds_after_its_last_consumer_finishes() {
    let channel = routed(json!({}));
    let first = event(&channel, "msg-1", None);
    let second = event(&channel, "msg-1", Some(reviewer()));
    let mut groups = Groups::default();
    groups.join(id(&first), &channel, &first).0.terminal_at = None;
    let (group, reviewer_at) = groups.join(id(&second), &channel, &second);
    group.finish(0);
    assert_eq!(groups.entries(), 2);
    tokio::time::advance(Duration::from_secs(40)).await;
    groups.expire();
    assert_eq!(groups.entries(), 2, "the reviewer is still running");
    groups
        .join(id(&second), &channel, &second)
        .0
        .finish(reviewer_at);
    tokio::time::advance(Duration::from_secs(29)).await;
    groups.expire();
    assert_eq!(groups.entries(), 2);
    tokio::time::advance(Duration::from_secs(2)).await;
    groups.expire();
    assert_eq!(groups.entries(), 0);
}
