//! Coalesced, content-free hints; the durable store remains authoritative.
use crate::AgentId;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::watch;

pub struct AgentHeadSubscription {
    receiver: watch::Receiver<()>,
    _sender: Arc<watch::Sender<()>>,
}

impl AgentHeadSubscription {
    pub async fn recv(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }
}

#[derive(Default)]
pub(super) struct AgentHeadHub {
    lanes: Mutex<BTreeMap<AgentId, Weak<watch::Sender<()>>>>,
}

impl AgentHeadHub {
    pub(super) fn subscribe(&self, id: &AgentId) -> AgentHeadSubscription {
        let mut lanes = self.lanes.lock().unwrap_or_else(|error| error.into_inner());
        lanes.retain(|_, sender| sender.strong_count() > 0);
        let sender = lanes.get(id).and_then(Weak::upgrade).unwrap_or_else(|| {
            let sender = Arc::new(watch::channel(()).0);
            lanes.insert(id.clone(), Arc::downgrade(&sender));
            sender
        });
        AgentHeadSubscription {
            receiver: sender.subscribe(),
            _sender: sender,
        }
    }

    pub(super) fn publish(&self, id: &AgentId) {
        let sender = self
            .lanes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .and_then(Weak::upgrade);
        if let Some(sender) = sender {
            sender.send_replace(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn hints_are_per_agent_coalesced_and_retained_until_observed() {
        let hub = AgentHeadHub::default();
        let id = AgentId("fixture".into());
        let mut left = hub.subscribe(&id);
        let mut right = hub.subscribe(&id);
        let mut other = hub.subscribe(&AgentId("other".into()));
        for _ in 0..1000 {
            hub.publish(&id);
        }
        left.recv().await.unwrap();
        right.recv().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), left.recv())
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), other.recv())
                .await
                .is_err()
        );
        drop((left, right, other));
        hub.publish(&id);
        assert!(hub
            .lanes
            .lock()
            .unwrap()
            .values()
            .all(|lane| lane.strong_count() == 0));
        let _next = hub.subscribe(&AgentId("next".into()));
        assert_eq!(hub.lanes.lock().unwrap().len(), 1);
    }
}
