use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::AgentId;

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

fn id(suffix: &str) -> AgentId {
    AgentId(format!("agt_title_{suffix}"))
}

fn base_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000)
}

fn make_eligible(store: &InMemoryAgentTitleStore, id: &AgentId) {
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner".into(),
        },
        AgentLogKind::AgentMessage {
            content: "assistant".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ] {
        store.append_agent_log(id, NOW, kind).unwrap();
    }
}

#[tokio::test]
async fn candidate_paging_is_deterministic_and_bounded_by_raw_agents() {
    let store = InMemoryAgentTitleStore::default();
    let a = id("page_a");
    let b = id("page_b");
    let c = id("page_c");
    for id in [&a, &b, &c] {
        store.register_agent(id.clone()).unwrap();
    }
    make_eligible(&store, &b);

    let first = store
        .agent_title_candidates(&PageRequest {
            limit: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(first.items.is_empty());
    assert_eq!(first.next.as_deref(), Some(a.as_str()));
    let second = store
        .agent_title_candidates(&PageRequest {
            limit: 1,
            cursor: first.next,
        })
        .await
        .unwrap();
    assert_eq!(second.items, vec![b.clone()]);
    assert_eq!(second.next.as_deref(), Some(b.as_str()));
    let third = store
        .agent_title_candidates(&PageRequest {
            limit: 1,
            cursor: second.next,
        })
        .await
        .unwrap();
    assert!(third.items.is_empty());
    assert_eq!(third.next, None);
    assert_eq!(
        store
            .agent_title_candidates(&PageRequest {
                limit: 0,
                cursor: None,
            })
            .await,
        Err(AgentTitleStoreError::InvalidPage)
    );
}

#[tokio::test]
async fn concurrent_claimers_acquire_one_generation_authority() {
    let store = Arc::new(InMemoryAgentTitleStore::default());
    let id = id("concurrent_claim");
    store.register_agent(id.clone()).unwrap();
    make_eligible(&store, &id);
    let mut tasks = Vec::new();
    for index in 0..16 {
        let store = Arc::clone(&store);
        let id = id.clone();
        tasks.push(tokio::spawn(async move {
            store
                .acquire_agent_title_claim(
                    &AgentTitleClaimRequest::new(id, format!("worker-{index}")),
                    base_time(),
                )
                .await
                .unwrap()
        }));
    }
    let mut acquired = Vec::new();
    for task in tasks {
        if let Some(claim) = task.await.unwrap() {
            acquired.push(claim);
        }
    }
    assert_eq!(acquired.len(), 1);
    assert_eq!(acquired[0].mode(), AgentTitleClaimMode::Generate);
}

#[tokio::test]
async fn malformed_claims_sources_and_timestamps_are_rejected() {
    let store = InMemoryAgentTitleStore::default();
    let id = id("invalid_inputs");
    store.register_agent(id.clone()).unwrap();
    make_eligible(&store, &id);
    assert_eq!(
        store
            .acquire_agent_title_claim(&AgentTitleClaimRequest::new(id.clone(), ""), base_time())
            .await,
        Err(AgentTitleStoreError::InvalidClaim)
    );
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "valid-holder"),
            base_time(),
        )
        .await
        .unwrap()
        .unwrap();
    let title = AgentTitle::new("Validated Title").unwrap();
    assert_eq!(
        store
            .complete_agent_title(&claim, &title, AgentTitleSource::User, "not-a-time".into(),)
            .await,
        Err(AgentTitleStoreError::InvalidClaim)
    );
    assert_eq!(
        store
            .complete_agent_title(
                &claim,
                &title,
                AgentTitleSource::Generated,
                "not-a-time".into(),
            )
            .await,
        Err(AgentTitleStoreError::InvalidTimestamp)
    );
    assert_eq!(
        store
            .set_user_agent_title(&id, &title, "not-a-time".into())
            .await,
        Err(AgentTitleStoreError::InvalidTimestamp)
    );
}

#[tokio::test]
async fn retained_fallback_stays_visible_until_one_fenced_generated_title_replaces_it() {
    let store = InMemoryAgentTitleStore::default();
    let id = id("fallback_upgrade");
    store.register_agent(id.clone()).unwrap();
    make_eligible(&store, &id);
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "original"),
            base_time(),
        )
        .await
        .unwrap()
        .unwrap();
    let fallback = AgentTitle::new("hi，我想做个游戏").unwrap();
    store
        .complete_agent_title(&claim, &fallback, AgentTitleSource::Fallback, NOW.into())
        .await
        .unwrap();
    assert_eq!(
        store
            .agent_title_candidates(&PageRequest::default())
            .await
            .unwrap()
            .items,
        vec![id.clone()]
    );
    let retry = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "retry"),
            base_time() + Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.mode(), AgentTitleClaimMode::Generate);
    assert_eq!(
        store.agent_title(&id).await.unwrap().unwrap().title,
        fallback
    );
    assert!(store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(id.clone(), "other"),
            base_time() + Duration::from_secs(61)
        )
        .await
        .unwrap()
        .is_none());
    let generated = AgentTitle::new("European cottage game").unwrap();
    assert!(store
        .complete_agent_title(&retry, &generated, AgentTitleSource::Generated, NOW.into())
        .await
        .unwrap());
    assert_eq!(
        store.agent_title(&id).await.unwrap().unwrap().title,
        generated
    );
    assert!(store
        .agent_title_candidates(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(store.title_event_count(&id).unwrap(), 2);
}
