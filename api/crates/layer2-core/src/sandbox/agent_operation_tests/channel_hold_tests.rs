use super::command_work_tests::{fixture, request};
use super::fixtures::*;
use super::ledger_tests::reservation;
use crate::sandbox::{agent_io::*, AgentStatus};

fn channel_actor() -> AgentActor {
    AgentActor::Channel {
        bridge_id: "bridge".into(),
        grant_id: "grant".into(),
        platform: "telegram".into(),
        chat_id: "actual-chat".into(),
        sender_id: "actual-sender".into(),
    }
}

#[tokio::test]
async fn generic_admission_refuses_channel_actor_or_source_before_duplicate_replay() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let ordinary = reservation(&agent, &parent, "same-key");
    store.reserve_command(ordinary.clone()).await.unwrap();
    for actor in [false, true] {
        let mut unproved = ordinary.clone();
        if actor {
            unproved.envelope.actor = channel_actor();
        } else {
            unproved.envelope.source.kind = AgentSourceKind::Channel;
        }
        assert_eq!(
            store.reserve_command(unproved).await,
            Err(CommandLedgerError::Invalid)
        );
    }
    assert!(store.reserve_command(ordinary).await.is_ok());
}

#[tokio::test]
async fn channel_hold_survives_restart_and_blocks_claims_even_after_queue_expiry() {
    let (_, agent, parent) = fixture(AgentStatus::Ready).await;
    let ordinary = reservation(&agent, &parent, "held");
    let ordinary_record = ordinary.record(1, 0, None).unwrap();
    let ordinary_wire = serde_json::to_value(&ordinary_record).unwrap();
    assert_eq!(ordinary_record.status, AgentResponseStatus::Queued);
    let mut channel = ordinary;
    channel.envelope.actor = channel_actor();
    channel.envelope.source.kind = AgentSourceKind::Channel;
    let original = channel.record(1, 0, None).unwrap();
    let mut retained: AdmittedAgentCommand =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(retained, original);
    assert_eq!(retained.status, AgentResponseStatus::WaitingAction);
    assert_eq!(
        serde_json::to_value(&retained).unwrap()["work"],
        ordinary_wire["work"]
    );
    assert_eq!(retained.work, CommandWorkState::default());
    for now in [NOW, LATEST, "2099-01-01T00:00:00Z"] {
        assert!(!retained.awaiting_claim(now));
        assert!(!command_ready_for_claim(&retained, Some(&agent), now));
        assert!(!command_ready_for_claim(&retained, None, now));
        assert_eq!(
            retained.claim(&request(&agent, "generic-worker", now), 1),
            Err(CommandLedgerError::Conflict)
        );
        assert_eq!(
            retained.reject_unclaimed(1, CommandFailureCode::QueueTimeout, now.into()),
            Err(CommandLedgerError::StaleClaim)
        );
    }
    assert_eq!(retained, original);
}
