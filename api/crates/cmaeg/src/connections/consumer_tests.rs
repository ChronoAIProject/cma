//! Connection setup stays the default consumer's: an event_ref issued to any
//! other consumer is refused by name, before anything is read or stored.
use super::{
    model::{now, CreateConnection},
    tests::{owner, Fixture},
    Error,
};
use crate::{auth::AgentCaller, channels::routing_fixture};
use cmaeg_core::{
    ids::{ChannelId, ConsumerId, ConversationId},
    problem::ProblemCode,
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use sha2::{Digest, Sha256};

fn caller(consumer: &str) -> AgentCaller {
    AgentCaller {
        key_hash: Sha256::digest(routing_fixture::agent_key(consumer).as_bytes()).into(),
    }
}

fn request(reference: &EventRef) -> CreateConnection {
    CreateConnection {
        record_id: uuid::Uuid::new_v4().to_string(),
        event_ref: reference.as_str().into(),
        suggested_name: None,
    }
}

#[tokio::test]
async fn a_non_default_consumer_cannot_start_a_connection() {
    let fixture = Fixture::new().await;
    let mut value = routing_fixture::routed();
    value["record_id"] = uuid::Uuid::new_v4().to_string().into();
    let definition = routing_fixture::definition(value);
    definition.validate().unwrap();
    let channel: ChannelId = fixture.retain_channel(&definition);
    let event = |consumer: Option<ConsumerId>| {
        EventRef::seal(
            &EventRefPayload {
                v: 1,
                activity_sha256: None,
                channel_id: channel.clone(),
                record_id: definition.record_id.clone(),
                event_id: "fixture-event".parse().unwrap(),
                partition_key: "private-actor".into(),
                conversation_id: ConversationId::derive(&fixture.keys, &channel, 1, "actor"),
                expires_at: now() + 600,
                reply: ReplyAddress::None,
                consumer,
                lineage: None,
            },
            &fixture.keys,
        )
        .unwrap()
    };
    let reviewer = ConsumerId::parse(routing_fixture::REVIEWER).unwrap();
    let service = fixture.service();
    let alice = owner("alice", false);

    // The reviewer's own reference authorizes it, and is refused by name.
    let own = event(Some(reviewer.clone()));
    let refused = service
        .create(&alice, caller(routing_fixture::REVIEWER), request(&own))
        .await;
    assert!(
        matches!(refused, Err(Error::Forbidden("consumer_not_supported"))),
        "{refused:?}"
    );
    let problem = Error::Forbidden("consumer_not_supported").problem();
    assert_eq!(
        (problem.code, problem.status, problem.detail.as_deref()),
        (ProblemCode::Forbidden, 403, Some("consumer_not_supported"))
    );

    // Another consumer's key fails authorization, which connection setup
    // reports as not found, as it always has.
    let mismatched = service
        .create(&alice, caller("default"), request(&own))
        .await;
    assert!(matches!(mismatched, Err(Error::NotFound)), "{mismatched:?}");

    // The default consumer passes authorization and goes on to read the
    // event's context, which this fixture's empty provider registry cannot.
    let default = service
        .create(&alice, caller("default"), request(&event(None)))
        .await;
    assert!(matches!(default, Err(Error::Unavailable)), "{default:?}");
}
