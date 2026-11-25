use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct UnknownStore {
    inner: Arc<InMemoryBotStore>,
    settlement: bool,
    commit: bool,
    fired: AtomicBool,
}
#[async_trait::async_trait]
impl BotStore for UnknownStore {
    async fn insert(&self, bot: &Bot) -> Result<(), BotError> {
        self.inner.insert(bot).await
    }
    async fn get(&self, id: &str) -> Result<Option<Bot>, BotError> {
        self.inner.get(id).await
    }
    async fn creation(&self, owner: &crate::SubjectId, key: &str) -> Result<Option<Bot>, BotError> {
        self.inner.creation(owner, key).await
    }
    async fn list(
        &self,
        owner: &crate::SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        self.inner.list(owner, after, limit).await
    }
    async fn pending(&self, after: Option<&str>, limit: usize) -> Result<Vec<Bot>, BotError> {
        self.inner.pending(after, limit).await
    }
    async fn replace(&self, revision: u64, bot: &Bot) -> Result<(), BotError> {
        let prior = self.inner.get(&bot.bot_id).await?.unwrap();
        let had = prior
            .work
            .as_ref()
            .is_some_and(|work| work.effect.is_some());
        let has = bot.work.as_ref().is_some_and(|work| work.effect.is_some());
        let boundary = if self.settlement {
            had && !has
        } else {
            !had && has
        };
        if boundary && !self.fired.swap(true, Ordering::SeqCst) {
            if self.commit {
                self.inner.replace(revision, bot).await?;
            }
            return Err(BotError::Unavailable);
        }
        self.inner.replace(revision, bot).await
    }
}

#[tokio::test]
async fn reservation_and_settlement_unknowns_never_start_compensating_effects() {
    for settlement in [false, true] {
        for commit in [false, true] {
            let f = Fixture::fenced().await;
            let store = Arc::new(UnknownStore {
                inner: f.store.clone(),
                settlement,
                commit,
                fired: AtomicBool::new(false),
            });
            let service = BotService::new(
                store.clone(),
                Arc::new(tests::Profiles),
                "registered".into(),
            )
            .with_effect_reservations(true);
            let worker = BotWorker::new(service, f.external.clone(), "unknown-ack".into());
            assert_eq!(worker.run(&f.bot_id).await, Err(BotError::Unavailable));
            assert!(store.fired.load(Ordering::SeqCst));
            let bot = f.bot().await;
            let work = bot.work.as_ref().unwrap();
            assert_eq!(work.operation, BotOperation::Create);
            assert_eq!(bot.state, BotState::Creating);
            assert_eq!(
                f.external.0.lock().unwrap().keys.len(),
                usize::from(settlement)
            );
            assert_eq!(work.effect.is_some(), settlement != commit);
            assert_eq!(
                bot.resources.credentials.agent_key_id.is_some(),
                settlement && commit
            );
            let successor = BotService::new(
                f.store.clone(),
                Arc::new(tests::Profiles),
                "registered".into(),
            )
            .with_effect_reservations(true);
            let claimed = successor
                .claim(
                    &f.bot_id,
                    "successor",
                    SystemTime::now() + Duration::from_secs(3600),
                )
                .await
                .unwrap();
            assert_eq!(claimed.is_none(), work.effect.is_some());
            if let Some(claimed) = claimed {
                assert_eq!(claimed.work.unwrap().operation, BotOperation::Cleanup);
                assert_eq!(claimed.resources, bot.resources);
            }
        }
    }
}
