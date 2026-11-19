//! Shared-observation fixtures retain the real core release decisions.
use layer2_core::{
    resource_access::{ResourceAccessSnapshot, ResourceAccessStore, ResourceTarget},
    sandbox::{
        agent_observation::*,
        store::{InMemorySandboxStore, SandboxStoreError},
        Agent, AgentReadError, AgentReadService,
    },
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub(crate) struct Facts {
    pub store: Arc<InMemorySandboxStore>,
    pub unavailable: AtomicBool,
    pub missing: AtomicBool,
}
#[async_trait::async_trait]
impl AgentObservationStore for Facts {
    fn ready(&self) -> bool {
        true
    }
    async fn source_generation(&self, agent: &Agent) -> Result<Option<Vec<u8>>, SandboxStoreError> {
        Ok(Some(serde_json::to_vec(agent).unwrap()))
    }
    async fn fresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<Result<ResourceAccessSnapshot, AgentReadError>>, AgentReadError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AgentReadError::Unavailable("fixture read timeout".into()));
        }
        let mut rows = Vec::new();
        for token in tokens {
            if self.missing.load(Ordering::SeqCst) {
                rows.push(Err(AgentReadError::NotFound));
                continue;
            }
            rows.push(
                self.store
                    .resource_access_snapshot(&ResourceTarget::Agent {
                        agent_id: token.agent_id.clone(),
                    })
                    .await
                    .map_err(|error| AgentReadError::Access(error.into())),
            );
        }
        Ok(rows)
    }
    async fn refresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError> {
        Ok(tokens
            .iter()
            .cloned()
            .zip(self.fresh(tokens).await?)
            .map(|(token, snapshot)| AgentObservationRefresh {
                token,
                until: tokio::time::Instant::now() + Duration::from_secs(15),
                snapshot: Some(snapshot),
            })
            .collect())
    }
}
pub(crate) fn attach(
    reads: &Arc<AgentReadService>,
    store: Arc<InMemorySandboxStore>,
) -> (Arc<AgentReadService>, Arc<Facts>) {
    let facts = Arc::new(Facts {
        store,
        unavailable: AtomicBool::new(false),
        missing: AtomicBool::new(false),
    });
    (
        reads.with_observation(AgentObservationService::new(reads.clone(), facts.clone())),
        facts,
    )
}
pub(crate) fn custody(agent: &Agent) -> Arc<AgentObservationAuthority> {
    AgentObservationAuthority::new(
        AgentObservationToken {
            agent_id: agent.id.clone(),
            created_at: agent.created_at.clone(),
            instance_id: "fixture".into(),
            epoch: 1,
        },
        tokio::time::Instant::now() + Duration::from_secs(15),
    )
}
