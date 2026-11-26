use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Generator {
    calls: Arc<AtomicUsize>,
    fail: bool,
    started: Option<Arc<tokio::sync::Notify>>,
}
struct Prepared {
    calls: Arc<AtomicUsize>,
    fail: bool,
    started: Option<Arc<tokio::sync::Notify>>,
}
#[async_trait::async_trait]
impl AgentTitleGenerator for Generator {
    async fn generate(
        &self,
        _: AgentTitleGenerationRequest,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        panic!("custody cannot use legacy generation")
    }
    async fn prepare(
        &self,
        _: AgentTitleGenerationRequest,
    ) -> Result<Box<dyn PreparedAgentTitle>, AgentTitleGenerationError> {
        Ok(Box::new(Prepared {
            calls: self.calls.clone(),
            fail: self.fail,
            started: self.started.clone(),
        }))
    }
}
#[async_trait::async_trait]
impl PreparedAgentTitle for Prepared {
    fn request_digest(&self) -> &str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
    fn strategy(&self) -> &str {
        "unit-strategy-v1"
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(started) = self.started {
            started.notify_one();
            return std::future::pending().await;
        }
        if self.fail {
            Err(AgentTitleGenerationError::unavailable())
        } else {
            Ok(AgentTitleGenerationResponse::new(
                "Accepted generated title",
            ))
        }
    }
}
fn fixture() -> (
    Arc<InMemoryAgentTitleStore>,
    Arc<AgentTitleService>,
    crate::AgentId,
) {
    let store = Arc::new(InMemoryAgentTitleStore::default().with_title_effect_custody(true));
    let id = crate::AgentId("agt_title_custody_unit".into());
    store.register_agent(id.clone()).unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner request".into(),
        },
        AgentLogKind::AgentMessage {
            content: "assistant answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ] {
        store
            .append_agent_log(&id, "2026-11-14T00:00:00Z", kind)
            .unwrap();
    }
    let service = AgentTitleService::new(store.clone(), "first").unwrap();
    (store, service, id)
}
fn context() -> AgentTitleGenerationContext {
    AgentTitleGenerationContext::new("unit-model", "unit-title-credential").unwrap()
}
#[tokio::test]
async fn title_effect_success_is_stored_before_publication_and_not_repeated() {
    let (store, service, id) = fixture();
    let calls = Arc::new(AtomicUsize::new(0));
    let generator = Generator {
        calls: calls.clone(),
        fail: false,
        started: None,
    };
    assert_eq!(
        service
            .generate_agent_title(&id, Some(&context()), Some(&generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    assert!(store
        .agent_title_effect(&id)
        .await
        .unwrap()
        .accepted()
        .is_some());
    assert_eq!(
        service
            .generate_agent_title(&id, Some(&context()), Some(&generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.title_event_count(&id).unwrap(), 1);
}
#[tokio::test]
async fn title_effect_failure_survives_off_and_arbitrarily_later_claims() {
    let (store, service, id) = fixture();
    let calls = Arc::new(AtomicUsize::new(0));
    let generator = Generator {
        calls: calls.clone(),
        fail: true,
        started: None,
    };
    assert_eq!(
        service
            .generate_agent_title(&id, Some(&context()), Some(&generator))
            .await,
        Err(AgentTitleServiceError::EffectUnknown)
    );
    let off = Arc::new((*store).clone().with_title_effect_custody(false));
    let successor = AgentTitleService::new(off.clone(), "successor").unwrap();
    assert_eq!(
        successor
            .generate_agent_title_at(
                &id,
                Some(&context()),
                Some(&generator),
                std::time::SystemTime::now() + std::time::Duration::from_secs(1_000_000)
            )
            .await
            .unwrap(),
        AgentTitleOutcome::Fallback
    );
    // The fallback sends nothing: the effect stays unresolved, with no redispatch (#2266).
    assert!(off.agent_title_effect(&id).await.unwrap().unknown());
    assert_eq!(
        off.agent_title(&id).await.unwrap().unwrap().source,
        AgentTitleSource::Fallback
    );
    assert_eq!(
        successor
            .generate_agent_title_at(
                &id,
                Some(&context()),
                Some(&generator),
                std::time::SystemTime::now() + std::time::Duration::from_secs(2_000_000)
            )
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn title_effect_cancellation_retains_uncertainty_without_redispatch() {
    let (store, service, id) = fixture();
    let calls = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let generator = Generator {
        calls: calls.clone(),
        fail: false,
        started: Some(started.clone()),
    };
    let task = {
        let id = id.clone();
        tokio::spawn(async move {
            service
                .generate_agent_title(&id, Some(&context()), Some(&generator))
                .await
        })
    };
    started.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(store.agent_title_effect(&id).await.unwrap().unknown());
    let next = AgentTitleService::new(store.clone(), "next").unwrap();
    let generator = Generator {
        calls: calls.clone(),
        fail: false,
        started: None,
    };
    assert_eq!(
        next.generate_agent_title_at(
            &id,
            Some(&context()),
            Some(&generator),
            std::time::SystemTime::now() + std::time::Duration::from_secs(1_000_000)
        )
        .await
        .unwrap(),
        AgentTitleOutcome::Fallback
    );
    assert!(store.agent_title_effect(&id).await.unwrap().unknown());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn title_effect_lost_completion_ack_does_not_lose_accepted_result() {
    let (store, service, id) = fixture();
    store.lose_next_completion_response();
    let calls = Arc::new(AtomicUsize::new(0));
    let generator = Generator {
        calls: calls.clone(),
        fail: false,
        started: None,
    };
    assert_eq!(
        service
            .generate_agent_title(&id, Some(&context()), Some(&generator))
            .await,
        Err(AgentTitleServiceError::Store(
            AgentTitleStoreError::Unavailable
        ))
    );
    assert!(store
        .agent_title_effect(&id)
        .await
        .unwrap()
        .accepted()
        .is_some());
    assert_eq!(
        service
            .generate_agent_title(&id, Some(&context()), Some(&generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(store.title_event_count(&id).unwrap(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn title_effect_unqualified_generator_cannot_fall_through_to_legacy_send() {
    struct LegacyOnly(AtomicUsize);
    #[async_trait::async_trait]
    impl AgentTitleGenerator for LegacyOnly {
        async fn generate(
            &self,
            _: AgentTitleGenerationRequest,
        ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(AgentTitleGenerationResponse::new("Must never be requested"))
        }
    }
    let (store, service, id) = fixture();
    let generator = LegacyOnly(AtomicUsize::new(0));
    assert_eq!(
        service
            .generate_agent_title(&id, Some(&context()), Some(&generator))
            .await,
        Err(AgentTitleServiceError::GenerationUnavailable)
    );
    assert_eq!(generator.0.load(Ordering::SeqCst), 0);
    assert!(matches!(
        store.agent_title_effect(&id).await.unwrap(),
        AgentTitleEffectState::Ready
    ));
}
