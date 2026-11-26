use super::*;
use futures_util::{stream, StreamExt, TryStreamExt};
use layer2_core::{
    agent_ownership::{Authority, OwnershipError, OwnershipToken, RetirementIntent},
    sandbox::AgentWorkStep,
};

#[cfg(test)]
mod mongo_tests;

#[derive(Default)]
pub(super) struct IntentTimes {
    entries: BTreeMap<AgentId, Timers>,
}

struct Timers {
    token: OwnershipToken,
    cooperative: Option<Instant>,
    inactive: Option<Instant>,
}

impl IntentTimes {
    fn eligible(&mut self, authority: &Authority, intent: RetirementIntent, active: bool) -> bool {
        let timers = self
            .entries
            .entry(authority.token.agent_id.clone())
            .or_insert_with(|| Timers {
                token: authority.token.clone(),
                cooperative: None,
                inactive: None,
            });
        if timers.token != authority.token {
            *timers = Timers {
                token: authority.token.clone(),
                cooperative: None,
                inactive: None,
            };
        }
        let timer = match intent {
            RetirementIntent::Cooperative => &mut timers.cooperative,
            RetirementIntent::Inactive => &mut timers.inactive,
        };
        if !active {
            *timer = None;
            return false;
        }
        timer.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(60)
    }

    fn remove(&mut self, authority: &Authority) {
        if self
            .entries
            .get(&authority.token.agent_id)
            .is_some_and(|entry| entry.token == authority.token)
        {
            self.entries.remove(&authority.token.agent_id);
        }
    }

    fn retain(&mut self, owned: &[Arc<Authority>]) {
        self.entries
            .retain(|_, entry| owned.iter().any(|authority| authority.token == entry.token));
    }
}

impl AgentManager {
    pub(super) async fn service_retirements(
        &self,
        hints: Vec<Arc<Authority>>,
    ) -> Result<(), OwnershipError> {
        let owned = self.coordinator.owned().await;
        self.containment_times.lock().unwrap().retain(&owned);
        let pending = self.coordinator.cancelled_receivers().await;
        let cleanup = stream::iter(pending)
            .map(|authority| async move {
                self.registry.detach_owner(&authority.token);
                if self
                    .coordinator
                    .retirement_background(&authority)
                    .await?
                    .is_some()
                {
                    self.containment_times.lock().unwrap().remove(&authority);
                }
                Ok::<_, OwnershipError>(())
            })
            .buffer_unordered(4)
            .try_collect::<Vec<_>>();
        let discovery = async {
            let count = owned.len();
            let start = self.retired_cursor.fetch_add(4, Ordering::Relaxed) % count.max(1);
            let inspections = (0..count.min(4))
                .map(|index| {
                    (
                        owned[(start + index) % count].clone(),
                        RetirementIntent::Inactive,
                    )
                })
                .chain(
                    hints
                        .into_iter()
                        .take(4)
                        .map(|authority| (authority, RetirementIntent::Cooperative)),
                );
            stream::iter(inspections)
                .map(|(authority, intent)| async move {
                    if !authority.is_current() {
                        return Ok(());
                    }
                    let observed = tokio::time::timeout(
                        Duration::from_secs(2),
                        self.store.get_agent(&authority.token.agent_id),
                    )
                    .await;
                    let Ok(Ok(agent)) = observed else {
                        return Ok(());
                    };
                    let eligible = match intent {
                        RetirementIntent::Inactive => {
                            let inactive = agent.is_none_or(|agent| {
                                !agent.status.is_live() && agent.work.step == AgentWorkStep::None
                            });
                            self.containment_times
                                .lock()
                                .unwrap()
                                .eligible(&authority, intent, inactive)
                        }
                        RetirementIntent::Cooperative => {
                            let busy = agent.is_none_or(|agent| {
                                agent.conversation.active_turn.is_some()
                                    || agent.work.lease.is_some()
                            });
                            !busy
                                || self
                                    .containment_times
                                    .lock()
                                    .unwrap()
                                    .eligible(&authority, intent, true)
                        }
                    };
                    if eligible {
                        authority.begin_retirement(intent)?;
                        self.registry.detach_owner(&authority.token);
                    }
                    Ok::<_, OwnershipError>(())
                })
                .buffer_unordered(4)
                .try_collect::<Vec<_>>()
                .await
        };
        tokio::try_join!(cleanup, discovery, self.coordinator.poll_acquisitions())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn late_completion_cannot_clear_successor_intent_timing() {
        let old = Authority::new(
            OwnershipToken {
                agent_id: AgentId("agent".into()),
                instance_id: "manager".into(),
                epoch: 1,
                protocol: PROTOCOL_VERSION,
            },
            Instant::now() + Duration::from_secs(120),
        );
        let mut token = old.token.clone();
        token.epoch += 1;
        let next = Authority::new(token, Instant::now() + Duration::from_secs(120));
        let mut times = IntentTimes::default();
        assert!(!times.eligible(&old, RetirementIntent::Inactive, true));
        tokio::time::advance(Duration::from_secs(50)).await;
        assert!(!times.eligible(&next, RetirementIntent::Inactive, true));
        times.remove(&old);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(times.eligible(&next, RetirementIntent::Inactive, true));
        times.remove(&next);
        assert!(times.entries.is_empty());
    }
}
