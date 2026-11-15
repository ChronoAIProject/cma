//! Receiver-side expiry for the existing per-bot source lease and its effects.
use super::{journal_model::Lease, record_store::Store, DurableJournal, GatewayError};
use crate::{
    records::mongo::{LeaseCondition, ReplicaGuard},
    replicas::store::Write,
};
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;

pub(super) fn failure(error: crate::replicas::Error) -> GatewayError {
    match error {
        crate::replicas::Error::Busy
        | crate::replicas::Error::Lost
        | crate::replicas::Error::Expired => GatewayError::Authority,
        crate::replicas::Error::Corrupt | crate::replicas::Error::Capacity => {
            GatewayError::Protocol
        }
        crate::replicas::Error::Unavailable => GatewayError::Unavailable,
    }
}

impl Store {
    async fn source_guard(&self, lease: &Lease) -> Result<(String, u64), GatewayError> {
        let (authority, version) =
            super::journal::load_authority_version(self, &lease.channel_record).await?;
        if authority.revision != lease.revision
            || authority.owner != lease.owner
            || authority.bot_id != lease.bot_id
        {
            return Err(GatewayError::Authority);
        }
        Ok((lease.channel_record.clone(), version))
    }
    pub fn lease_ms(&self) -> i64 {
        if self.replica.is_some() {
            crate::replicas::config::LEASE.as_millis() as i64
        } else {
            super::journal_model::LEASE_MS
        }
    }

    pub async fn sample(&self) -> Result<(i64, Instant), GatewayError> {
        match &self.replica {
            Some(store) => {
                let clock = store.clock().await.map_err(failure)?;
                Ok((clock.millis, clock.started))
            }
            None => Ok(((self.clock)(), Instant::now())),
        }
    }

    pub async fn put_lease(
        &self,
        id: &str,
        lease: &Lease,
        expected: Option<u64>,
        condition: Option<LeaseCondition>,
    ) -> Result<u64, GatewayError> {
        match &self.replica {
            Some(store) => store
                .write(
                    id,
                    &lease.owner,
                    lease,
                    Write {
                        expected,
                        condition,
                        channel: Some(self.source_guard(lease).await?),
                        ..Default::default()
                    },
                )
                .await
                .map_err(failure),
            None => self.put(id, &lease.owner, lease, expected, None).await,
        }
    }
}

impl DurableJournal {
    pub(super) fn locally_live(&self) -> bool {
        self.store.replica.is_none() || Instant::now() < *self.deadline.read().unwrap()
    }

    pub(super) async fn save(&self, lease: &Lease, version: u64) -> Result<(), GatewayError> {
        if self.store.replica.is_none() {
            return self
                .store
                .put(&self.id, &self.authority.owner, lease, Some(version), None)
                .await
                .map(|_| ());
        }
        let (previous, current) = self.fenced().await?;
        if current != version {
            return Err(GatewayError::Authority);
        }
        let (now, started) = self.store.sample().await?;
        if !self.locally_live() {
            return Err(GatewayError::Authority);
        }
        self.store
            .put_lease(
                &self.id,
                lease,
                Some(version),
                Some(LeaseCondition::Live(previous.lease_until)),
            )
            .await?;
        *self.deadline.write().unwrap() =
            started + Duration::from_millis(lease.lease_until.saturating_sub(now).max(0) as u64);
        Ok(())
    }

    /// Staged content chunks are not accepted work. The root/ACK marker is
    /// accepted only by a transaction touching the captured source generation.
    pub(super) async fn write_owned<T: Serialize>(
        &self,
        id: &str,
        owner: &str,
        value: &T,
        expected: Option<u64>,
        expiry: Option<i64>,
    ) -> Result<u64, GatewayError> {
        let Some(store) = &self.store.replica else {
            return self.store.put(id, owner, value, expected, expiry).await;
        };
        if owner != self.authority.owner {
            return Err(GatewayError::Authority);
        }
        let (lease, version) = self.fenced().await?;
        let channel = self.store.source_guard(&lease).await?;
        if !self.locally_live() {
            return Err(GatewayError::Authority);
        }
        let guard = ReplicaGuard {
            id: self.id.clone(),
            owner: owner.into(),
            version,
            until_ms: lease.lease_until,
        };
        store
            .write(
                id,
                owner,
                value,
                Write {
                    expected,
                    retained_until: expiry,
                    guard: Some(&guard),
                    channel: Some(channel),
                    ..Default::default()
                },
            )
            .await
            .map_err(failure)
    }
}
