//! Captured remote epochs follow a Sandbox checkpoint into its committing transaction.
use super::{OwnershipError, OwnershipToken};
use crate::AgentId;
use std::{collections::BTreeMap, future::Future, sync::Mutex};

tokio::task_local! {
    static OBSERVED: Mutex<BTreeMap<AgentId, OwnershipToken>>;
}

pub async fn transport_observation_scope<F: Future>(operation: F) -> F::Output {
    OBSERVED.scope(Mutex::new(BTreeMap::new()), operation).await
}

pub fn observe_transport(token: OwnershipToken) -> Result<(), OwnershipError> {
    OBSERVED
        .try_with(|observed| {
            let mut observed = observed.lock().unwrap();
            if observed
                .get(&token.agent_id)
                .is_some_and(|previous| previous != &token)
                || observed.len() >= crate::sandbox::store::MAX_SANDBOX_LIFECYCLE_AGENTS
            {
                return Err(OwnershipError::Stale);
            }
            observed.insert(token.agent_id.clone(), token);
            Ok(())
        })
        .unwrap_or(Ok(()))
}

pub fn observed_transport(id: &AgentId) -> Option<OwnershipToken> {
    OBSERVED
        .try_with(|observed| observed.lock().unwrap().get(id).cloned())
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn observations_are_scoped_and_cannot_mix_two_epochs_for_one_capture() {
        let token = OwnershipToken {
            agent_id: AgentId("fixture".into()),
            instance_id: "one".into(),
            epoch: 1,
            protocol: super::super::PROTOCOL_VERSION,
        };
        assert_eq!(observed_transport(&token.agent_id), None);
        transport_observation_scope(async {
            observe_transport(token.clone()).unwrap();
            assert_eq!(observed_transport(&token.agent_id), Some(token.clone()));
            let mut next = token.clone();
            next.epoch += 1;
            assert_eq!(observe_transport(next), Err(OwnershipError::Stale));
            assert_eq!(observed_transport(&token.agent_id), Some(token.clone()));
        })
        .await;
        assert_eq!(observed_transport(&token.agent_id), None);
    }
}
