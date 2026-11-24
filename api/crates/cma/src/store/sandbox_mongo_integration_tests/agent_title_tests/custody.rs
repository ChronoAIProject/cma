//! Synthetic counting provider through independent Mongo clients and title services.
use super::*;
use layer2_core::sandbox::{
    AgentTitleClaim, AgentTitleEffectIntent, AgentTitleEffectState, AgentTitleGenerationContext,
    AgentTitleGenerationError, AgentTitleGenerationResponse, AgentTitleOutcome, AgentTitleService,
    AgentTitleServiceError, PreparedAgentTitle,
};
use std::sync::atomic::AtomicUsize;

mod dispatch;
mod fixtures;
mod receipts;
mod retention;
mod scheduled_refusal;
use fixtures::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_two_instances_dispatch_exactly_once() {
    let f = Fixture::new("two_instances").await;
    let provider = Provider::new(false).await;
    let ctx = context();
    let (a, b) = tokio::join!(
        f.service_a
            .generate_agent_title(&f.id, Some(&ctx), Some(&provider.generator)),
        f.service_b
            .generate_agent_title(&f.id, Some(&ctx), Some(&provider.generator)),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == AgentTitleOutcome::Generated)
            .count(),
        1
    );
    assert_eq!(provider.calls(), 1);
    assert!(f
        .a
        .agent_title_effect(&f.id)
        .await
        .unwrap()
        .accepted()
        .is_some());
    assert_eq!(
        f.b.agent_title(&f.id)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_str(),
        "Retained provider title"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_lost_provider_response_never_resends_after_restart_or_off() {
    let f = Fixture::new("lost_response").await;
    let provider = Provider::new(true).await;
    assert_eq!(
        f.service_a
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await,
        Err(AgentTitleServiceError::EffectUnknown)
    );
    assert_eq!(provider.calls(), 1);
    let original = f.row().await;
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    expire(&f.a, &f.id).await;
    // A restarted process, OFF rollback, a new strategy/model and a new claim
    // token all observe the same Agent-lifetime obligation.
    let restarted = peer(&f.name, false).await;
    let service = AgentTitleService::new(Arc::new(restarted), "restarted-off").unwrap();
    let changed =
        AgentTitleGenerationContext::new("changed-strategy-model", "test-title-credential")
            .unwrap();
    // The first later claim is fallback-only: it publishes the prompt-derived
    // title without a provider call (#2266). Every later claim is skipped.
    for (service, expected) in [
        (&service, AgentTitleOutcome::Fallback),
        (&f.service_b, AgentTitleOutcome::Skipped),
    ] {
        assert_eq!(
            service
                .generate_agent_title(&f.id, Some(&changed), Some(&provider.generator))
                .await
                .unwrap(),
            expected
        );
    }
    let retained = f.row().await;
    assert_eq!(
        retained.get("title_effect_custody"),
        original.get("title_effect_custody")
    );
    assert_eq!(provider.calls(), 1);
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    assert_eq!(
        f.a.agent_title(&f.id).await.unwrap().unwrap().source,
        AgentTitleSource::Fallback
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_crash_after_intent_before_dispatch_preserves_zero_calls() {
    let f = Fixture::new("intent_crash").await;
    let provider = Provider::new(false).await;
    let (claim, intent) = f.intent().await;
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    expire(&f.a, &f.id).await;
    assert!(!f
        .a
        .validate_agent_title_effect(&claim, &intent)
        .await
        .unwrap());
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Fallback
    );
    let mut changed = intent.clone();
    changed.strategy = "new-strategy".into();
    changed.request_digest = "b".repeat(64);
    assert!(!f
        .b
        .begin_agent_title_effect(&claim, &changed)
        .await
        .unwrap());
    assert!(f.a.agent_title_effect(&f.id).await.unwrap().unknown());
    assert_eq!(provider.calls(), 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_proven_preparation_refusal_can_retry() {
    let f = Fixture::new("prepare_retry").await;
    let provider = Provider::new(false).await;
    let wrong = AgentTitleGenerationContext::new("wrong-model", "test-title-credential").unwrap();
    assert_eq!(
        f.service_a
            .generate_agent_title(&f.id, Some(&wrong), Some(&provider.generator))
            .await,
        Err(AgentTitleServiceError::GenerationUnavailable)
    );
    assert!(matches!(
        f.a.agent_title_effect(&f.id).await.unwrap(),
        AgentTitleEffectState::Ready
    ));
    assert_eq!(provider.calls(), 0);
    expire(&f.a, &f.id).await;
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(provider.calls(), 1);
    f.cleanup().await;
}

// #2266: an unresolved provider effect must not leave the Agent untitled
// forever. Discovery keeps it until the prompt-derived fallback is stored,
// and the fallback never makes another provider call.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_unresolved_effect_is_rediscovered_only_until_its_fallback_is_stored() {
    let f = Fixture::new("unknown_fallback").await;
    let provider = Provider::new(true).await;
    assert_eq!(
        f.service_a
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await,
        Err(AgentTitleServiceError::EffectUnknown)
    );
    let page = PageRequest::new(100, None).unwrap();
    let candidate = |page: layer2_core::sandbox::store::Page<AgentId>| page.items.contains(&f.id);
    // The original capability's live lease still excludes any successor.
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, None, None)
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    expire(&f.a, &f.id).await;
    assert!(candidate(f.b.agent_title_candidates(&page).await.unwrap()));
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Fallback
    );
    let stored = f.a.agent_title(&f.id).await.unwrap().unwrap();
    assert_eq!(stored.source, AgentTitleSource::Fallback);
    assert!(f.a.agent_title_effect(&f.id).await.unwrap().unknown());
    assert!(!candidate(f.b.agent_title_candidates(&page).await.unwrap()));
    assert_eq!(
        f.service_a
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(provider.calls(), 1);
    f.cleanup().await;
}
