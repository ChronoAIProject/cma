use super::{bounded, Authority};
use layer3_adapter::owner_protocol::{TransportAuthority, TransportOwner};
use layer3_adapter::AdapterError;
use std::time::Duration;

#[async_trait::async_trait]
impl TransportAuthority for Authority {
    fn register_receiver(
        &self,
        receiver: std::sync::Arc<dyn layer3_adapter::owner_protocol::ReceiverRevocation>,
    ) -> layer3_adapter::Result<()> {
        let mut receivers = self.receivers.lock().unwrap();
        if !self.is_current() || receivers.len() >= 64 {
            return Err(stale());
        }
        receivers.insert(receiver.identity(), receiver);
        Ok(())
    }
    fn is_current(&self) -> bool {
        Authority::is_current(self)
    }
    fn owner(&self) -> TransportOwner {
        TransportOwner {
            protocol: self.token.protocol,
            agent_id: self.token.agent_id.to_string(),
            instance_id: self.token.instance_id.clone(),
            epoch: self.token.epoch,
        }
    }

    async fn check(&self) -> layer3_adapter::Result<()> {
        if !self.is_current() {
            return Err(stale());
        }
        let store = self.store.as_ref().ok_or_else(stale)?;
        if bounded(store.check(&self.token)).await.is_err() {
            self.revoke();
            return Err(stale());
        }
        if self.is_current() {
            Ok(())
        } else {
            Err(stale())
        }
    }

    async fn renew_for_challenge(&self) -> layer3_adapter::Result<()> {
        if !self.is_current() {
            return Err(stale());
        }
        let store = self.store.as_ref().ok_or_else(stale)?;
        let started = tokio::time::Instant::now();
        let result = async {
            let membership = bounded(store.membership()).await.map_err(|_| stale())?;
            let lease = bounded(store.renew(&self.token))
                .await
                .map_err(|_| stale())?;
            if lease.token != self.token {
                return Err(stale());
            }
            let remaining = lease
                .expires_at_ms
                .checked_sub(membership.now_ms)
                .filter(|remaining| *remaining > 0)
                .ok_or_else(stale)?;
            self.renew_until(started + Duration::from_millis(remaining as u64))
                .map_err(|_| stale())
        }
        .await;
        if result.is_err() {
            self.revoke();
        }
        result
    }

    async fn revoked(&self) {
        Authority::revoked(self).await;
    }
    fn revoke(&self) {
        Authority::revoke(self);
    }
}

impl Authority {
    /// Retain every contacted receiver until all revocations succeed. A cancelled
    /// or ambiguous confirmation may have installed authority at the runtime.
    pub async fn revoke_transports(&self) -> Result<(), super::OwnershipError> {
        self.revoke();
        let receivers: Vec<_> = self.receivers.lock().unwrap().values().cloned().collect();
        for receiver in receivers {
            receiver.revoke().await.map_err(|_| {
                super::OwnershipError::Unavailable("receiving runtime revocation failed".into())
            })?;
        }
        self.receivers.lock().unwrap().clear();
        Ok(())
    }
}

fn stale() -> AdapterError {
    AdapterError::Rejected("Agent Manager transport ownership is stale or unavailable".into())
}
