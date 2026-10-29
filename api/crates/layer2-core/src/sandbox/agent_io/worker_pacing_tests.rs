use super::*;
use crate::SubjectId;

fn envelope() -> AgentCommandEnvelope {
    let command = AgentCommand::message("queued before runtime");
    AgentCommandEnvelope {
        continuation: None,
        schema_version: AGENT_IO_SCHEMA_VERSION,
        command_id: AgentCommandId::generate(),
        agent_id: crate::AgentId::generate(),
        owner: AgentOwner::Person {
            subject: SubjectId::new("owner"),
        },
        actor: AgentActor::Human {
            subject: SubjectId::new("owner"),
        },
        source: AgentCommandSource {
            kind: AgentSourceKind::Api,
            reference: "test".into(),
        },
        correlation_id: "test".into(),
        request_fingerprint: command.fingerprint().unwrap(),
        authorization: AgentAuthorizationRevision {
            authority_id: "fixture".into(),
            policy_revision: 1,
            grant_revision: None,
        },
        command,
    }
}

#[test]
fn fresh_api_message_rechecks_are_bounded_by_immutable_admission_age() {
    let original = envelope();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_537_600);
    let admitted = rfc3339_at(base);
    let deadline = rfc3339_at(base + Duration::from_secs(300));
    for (age, seconds) in [
        (0, 1),
        (29, 1),
        (30, 5),
        (119, 5),
        (120, 15),
        (299, 15),
        (300, 15),
    ] {
        let now = rfc3339_at(base + Duration::from_secs(age));
        assert_eq!(
            api_message_recheck_delay(&original, &admitted, &deadline, &now),
            Duration::from_secs(seconds)
        );
    }
    let future = rfc3339_at(base + Duration::from_secs(1));
    for (at, end, now) in [
        (future.as_str(), deadline.as_str(), admitted.as_str()),
        (admitted.as_str(), admitted.as_str(), admitted.as_str()),
        ("invalid", deadline.as_str(), admitted.as_str()),
        (admitted.as_str(), "invalid", admitted.as_str()),
        (admitted.as_str(), deadline.as_str(), "invalid"),
    ] {
        assert_eq!(
            api_message_recheck_delay(&original, at, end, now),
            Duration::from_secs(15)
        );
    }
}

#[test]
fn fast_claim_rechecks_preserve_controls_and_other_sources() {
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_788_537_600);
    let at = rfc3339_at(base);
    let deadline = rfc3339_at(base + Duration::from_secs(300));
    let mut api = envelope();
    api.actor = AgentActor::Api {
        subject: SubjectId::new("owner"),
        delegation_id: None,
    };
    assert_eq!(
        api_message_recheck_delay(&api, &at, &deadline, &at),
        Duration::from_secs(1)
    );
    for source in [
        AgentSourceKind::UserInterface,
        AgentSourceKind::Trigger,
        AgentSourceKind::Channel,
    ] {
        let mut other = api.clone();
        other.source.kind = source;
        assert_eq!(
            api_message_recheck_delay(&other, &at, &deadline, &at),
            Duration::from_secs(15)
        );
    }
    let mut channel = api.clone();
    channel.actor = AgentActor::Channel {
        bridge_id: "bridge".into(),
        grant_id: "grant".into(),
        platform: "platform".into(),
        chat_id: "chat".into(),
        sender_id: "sender".into(),
    };
    assert_eq!(
        api_message_recheck_delay(&channel, &at, &deadline, &at),
        Duration::from_secs(15)
    );
    api.command = AgentCommand::Interrupt;
    assert_eq!(
        api_message_recheck_delay(&api, &at, &deadline, &at),
        Duration::from_secs(15)
    );
}
