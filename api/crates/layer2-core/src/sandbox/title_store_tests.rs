use std::time::{Duration, SystemTime};

use crate::AgentId;

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";
const LATER: &str = "2026-09-05T00:00:01Z";

fn id(suffix: &str) -> AgentId {
    AgentId(format!("agt_title_{suffix}"))
}

fn base_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000)
}

fn append(store: &InMemoryAgentTitleStore, id: &AgentId, kinds: Vec<AgentLogKind>) {
    for kind in kinds {
        store.append_agent_log(id, NOW, kind).unwrap();
    }
}

fn completed_turn(owner: &str, assistant: &str) -> Vec<AgentLogKind> {
    vec![
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: owner.into(),
        },
        AgentLogKind::AgentMessage {
            content: assistant.into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ]
}

fn failure() -> TypedFailure {
    TypedFailure {
        class: "runtime_lost".into(),
        detail: "turn did not complete".into(),
    }
}

#[tokio::test]
async fn only_ordered_complete_first_turn_evidence_is_eligible() {
    let cases = vec![
        vec![],
        vec![AgentLogKind::OwnerMessage {
            content: "owner only".into(),
        }],
        vec![AgentLogKind::AgentMessage {
            content: "agent only".into(),
        }],
        vec![AgentLogKind::TurnCompleted { turn_sequence: 1 }],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "missing agent".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 1 },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "missing completion".into(),
            },
            AgentLogKind::AgentMessage {
                content: "not enough".into(),
            },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "failed".into(),
            },
            AgentLogKind::AgentMessage {
                content: "partial".into(),
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 1,
                failure: failure(),
            },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "interrupted".into(),
            },
            AgentLogKind::AgentMessage {
                content: "partial".into(),
            },
            AgentLogKind::TurnInterrupted { turn_sequence: 1 },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "mismatch".into(),
            },
            AgentLogKind::AgentMessage {
                content: "wrong turn".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 2 },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "failed first owner".into(),
            },
            AgentLogKind::AgentMessage {
                content: "failed first agent".into(),
            },
            AgentLogKind::TurnFailed {
                turn_sequence: 1,
                failure: failure(),
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 2,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "later owner cannot rescue".into(),
            },
            AgentLogKind::AgentMessage {
                content: "later agent cannot rescue".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 2 },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::AgentMessage {
                content: "out of order".into(),
            },
            AgentLogKind::OwnerMessage {
                content: "too late".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 1 },
        ],
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 1,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "nested".into(),
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 2,
                origin_key: None,
            },
            AgentLogKind::AgentMessage {
                content: "wrong region".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 1 },
        ],
    ];
    let store = InMemoryAgentTitleStore::default();
    for (index, kinds) in cases.into_iter().enumerate() {
        let id = id(&format!("ineligible_{index:02}"));
        store.register_agent(id.clone()).unwrap();
        append(&store, &id, kinds);
        assert!(store.agent_title_transcript(&id).await.unwrap().is_none());
        assert!(store
            .acquire_agent_title_claim(&AgentTitleClaimRequest::new(id, "worker-a"), base_time(),)
            .await
            .unwrap()
            .is_none());
    }

    let eligible = id("eligible");
    store.register_agent(eligible.clone()).unwrap();
    append(
        &store,
        &eligible,
        completed_turn("first owner", "first assistant"),
    );
    append(
        &store,
        &eligible,
        completed_turn("later owner", "later assistant"),
    );
    let transcript = store
        .agent_title_transcript(&eligible)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transcript.turn_sequence(), 1);
    assert_eq!(transcript.owner_message(), "first owner");
    assert_eq!(transcript.assistant_message(), "first assistant");
    let candidates = store
        .agent_title_candidates(&PageRequest::default())
        .await
        .unwrap();
    assert_eq!(candidates.items, vec![eligible]);

    let non_one = id("eligible_non_one_sequence");
    store.register_agent(non_one.clone()).unwrap();
    append(
        &store,
        &non_one,
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 7,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "recovered owner".into(),
            },
            AgentLogKind::AgentMessage {
                content: "recovered assistant".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 7 },
        ],
    );
    assert_eq!(
        store
            .agent_title_transcript(&non_one)
            .await
            .unwrap()
            .unwrap()
            .turn_sequence(),
        7
    );
}

#[tokio::test]
async fn claim_expiry_is_45_seconds_and_takeover_is_fallback_only() {
    let store = InMemoryAgentTitleStore::default();
    let id = id("takeover");
    store.register_agent(id.clone()).unwrap();
    append(&store, &id, completed_turn("Takeover owner", "response"));
    let first_request = AgentTitleClaimRequest::new(id.clone(), "holder-a");
    let first = store
        .acquire_agent_title_claim(&first_request, base_time())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.mode(), AgentTitleClaimMode::Generate);
    assert_eq!(
        first
            .expires_at()
            .duration_since(first.acquired_at())
            .unwrap(),
        Duration::from_secs(45)
    );
    assert_eq!(
        store
            .acquire_agent_title_claim(&first_request, base_time() + Duration::from_secs(44),)
            .await
            .unwrap(),
        Some(first.clone())
    );
    assert!(store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "holder-b"),
            base_time() + Duration::from_secs(44),
        )
        .await
        .unwrap()
        .is_none());

    let successor = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "holder-b"),
            base_time() + Duration::from_secs(45),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.mode(), AgentTitleClaimMode::FallbackOnly);
    let generated = AgentTitle::new("Stale Generated Title").unwrap();
    assert!(!store
        .complete_agent_title(
            &successor,
            &generated,
            AgentTitleSource::Generated,
            LATER.into(),
        )
        .await
        .unwrap());
    assert!(!store
        .complete_agent_title(
            &first,
            &generated,
            AgentTitleSource::Generated,
            LATER.into(),
        )
        .await
        .unwrap());

    let fallback = AgentTitle::new("Takeover owner").unwrap();
    assert!(store
        .complete_agent_title(
            &successor,
            &fallback,
            AgentTitleSource::Fallback,
            LATER.into(),
        )
        .await
        .unwrap());
    assert!(store
        .complete_agent_title(
            &successor,
            &generated,
            AgentTitleSource::Generated,
            LATER.into(),
        )
        .await
        .unwrap());
    assert_eq!(store.title_event_count(&id).unwrap(), 1);
    let stored = store.agent_title(&id).await.unwrap().unwrap();
    assert_eq!(stored.title, fallback);
    assert_eq!(stored.source, AgentTitleSource::Fallback);
}

#[tokio::test]
async fn completion_requires_exact_holder_and_token_and_replays_lost_response() {
    let store = InMemoryAgentTitleStore::default();
    let id = id("completion");
    store.register_agent(id.clone()).unwrap();
    append(&store, &id, completed_turn("Completion owner", "response"));
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "right-holder"),
            base_time(),
        )
        .await
        .unwrap()
        .unwrap();
    let wrong_request = AgentTitleClaimRequest::new(id.clone(), "wrong-holder");
    let wrong = AgentTitleClaim::acquired(
        wrong_request,
        AgentTitleClaimMode::Generate,
        claim.acquired_at(),
        claim.expires_at(),
    );
    let title = AgentTitle::new("Committed Once").unwrap();
    assert!(!store
        .complete_agent_title(&wrong, &title, AgentTitleSource::Generated, LATER.into(),)
        .await
        .unwrap());
    let wrong_token = AgentTitleClaim::acquired(
        AgentTitleClaimRequest::new(id.clone(), "right-holder"),
        AgentTitleClaimMode::Generate,
        claim.acquired_at(),
        claim.expires_at(),
    );
    assert!(!store
        .complete_agent_title(
            &wrong_token,
            &title,
            AgentTitleSource::Generated,
            LATER.into(),
        )
        .await
        .unwrap());

    store.lose_next_completion_response();
    assert_eq!(
        store
            .complete_agent_title(&claim, &title, AgentTitleSource::Generated, LATER.into(),)
            .await,
        Err(AgentTitleStoreError::Unavailable)
    );
    assert!(store
        .complete_agent_title(&claim, &title, AgentTitleSource::Generated, LATER.into(),)
        .await
        .unwrap());
    assert_eq!(store.title_event_count(&id).unwrap(), 1);
}

#[tokio::test]
async fn user_title_is_write_once_and_wins_only_when_it_commits_first() {
    let store = InMemoryAgentTitleStore::default();
    let first_id = id("user_first");
    store.register_agent(first_id.clone()).unwrap();
    append(
        &store,
        &first_id,
        completed_turn("Owner fallback", "response"),
    );
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(first_id.clone(), "worker"),
            base_time(),
        )
        .await
        .unwrap()
        .unwrap();
    let user = AgentTitle::new("User Chosen Title").unwrap();
    assert_eq!(
        store
            .set_user_agent_title(&first_id, &user, LATER.into())
            .await
            .unwrap(),
        SetAgentTitleOutcome::Set
    );
    assert!(!store
        .complete_agent_title(
            &claim,
            &AgentTitle::new("Generated Too Late").unwrap(),
            AgentTitleSource::Generated,
            LATER.into(),
        )
        .await
        .unwrap());
    assert!(store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(first_id.clone(), "other"),
            base_time() + Duration::from_secs(60),
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.agent_title(&first_id).await.unwrap().unwrap().title,
        user
    );

    let generated_first = id("generated_first");
    store.register_agent(generated_first.clone()).unwrap();
    append(
        &store,
        &generated_first,
        completed_turn("Owner", "response"),
    );
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(generated_first.clone(), "worker"),
            base_time(),
        )
        .await
        .unwrap()
        .unwrap();
    let generated = AgentTitle::new("Generated First").unwrap();
    assert!(store
        .complete_agent_title(
            &claim,
            &generated,
            AgentTitleSource::Generated,
            LATER.into(),
        )
        .await
        .unwrap());
    assert_eq!(
        store
            .set_user_agent_title(&generated_first, &user, LATER.into())
            .await
            .unwrap(),
        SetAgentTitleOutcome::AlreadySet
    );
    assert_eq!(
        store
            .agent_title(&generated_first)
            .await
            .unwrap()
            .unwrap()
            .title,
        generated
    );
}
