use super::{payload::Root, repository::*, Config, Error};
use crate::activity::{Change, ChannelActivity};
use cmaeg_core::seal::MasterKeyRing;
use std::sync::Arc;

#[derive(Clone)]
pub struct ActivityHistory {
    pub(super) repository: Option<Arc<dyn ActivityHistoryRepository>>,
    pub(super) keys: Arc<MasterKeyRing>,
    pub(super) config: Config,
}
impl ActivityHistory {
    pub fn new(
        repository: Option<Arc<dyn ActivityHistoryRepository>>,
        keys: Arc<MasterKeyRing>,
        config: Config,
    ) -> Self {
        Self {
            repository,
            keys,
            config,
        }
    }
    pub(super) fn reader(&self) -> Result<&dyn ActivityHistoryRepository, Error> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }
        self.repository.as_deref().ok_or(Error::UnsupportedBackend)
    }
    pub async fn received(
        &self,
        owner: &str,
        record: &str,
        mut row: ChannelActivity,
    ) -> Result<(), Error> {
        if !self.config.capture {
            return Ok(());
        }
        let repo = self.reader()?;
        let boundary = repo.boundary().await?;
        if boundary.since_ms.is_some() && boundary.retention_days != self.config.retention_days {
            return Err(Error::InvalidConfiguration);
        }
        let scope = super::scope(owner, record);
        let event_key = super::event_key(&scope, &row.event_id);
        let received_ms = time::OffsetDateTime::parse(
            &row.received_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| Error::InvalidRequest)?
        .unix_timestamp_nanos()
            / 1_000_000;
        let received_ms = i64::try_from(received_ms).map_err(|_| Error::InvalidRequest)?;
        let now = super::now_ms();
        let expires_ms = received_ms
            .checked_add(self.config.retention_ms())
            .ok_or(Error::InvalidRequest)?;
        if expires_ms <= now || received_ms > now + 60_000 {
            return Err(Error::Expired);
        }
        row.received_at = super::paging::format_time(received_ms)?;
        row.consumers.clear();
        let root = Root {
            schema: 1,
            scope: scope.clone(),
            event_key: event_key.clone(),
            received_ms,
            expires_ms,
            receipt: row.clone(),
            inline: Some(row),
            manifest: None,
            legacy_partial: false,
            detail_incomplete: false,
            detail_truncated: false,
        };
        let stored = repo
            .insert(StoredEvent {
                scope,
                key: Position {
                    received_ms,
                    event_key,
                },
                sequence: 0,
                version: 1,
                expires_ms,
                blob: root.encode(&self.keys)?,
            })
            .await?;
        // Do not accept an existing opaque row without checking its sealed binding.
        Root::decode(&self.keys, &stored)?;
        metrics::counter!("cmaeg_activity_history_receipts_total").increment(1);
        Ok(())
    }
    pub async fn change(
        &self,
        owner: &str,
        record: &str,
        event: &str,
        change: &Change,
    ) -> Result<(), Error> {
        if !self.config.capture {
            return Ok(());
        }
        let repo = self.reader()?;
        let scope = super::scope(owner, record);
        let key = super::event_key(&scope, event);
        for _ in 0..8 {
            let Some(mut stored) = repo.get(&scope, &key, super::now_ms()).await? else {
                return Ok(());
            };
            let mut root = Root::decode(&self.keys, &stored)?;
            let mut summary = match root.summary(repo, &self.keys, super::now_ms()).await {
                Ok(summary) => summary,
                Err(error) => {
                    root.detail_incomplete = true;
                    stored.version += 1;
                    stored.blob = root.encode(&self.keys)?;
                    let expected = stored.version - 1;
                    if repo.compare_exchange(stored, expected).await? {
                        return Err(error);
                    }
                    continue;
                }
            };
            let before = serde_json::to_vec(&summary).map_err(|_| Error::Unreadable)?;
            change.apply(&mut summary);
            if serde_json::to_vec(&summary).map_err(|_| Error::Unreadable)? == before {
                return Ok(());
            }
            // These are bounded current summaries, not exhaustive transition logs.
            root.detail_truncated |= summary.consumers.len() >= 16
                || summary
                    .consumers
                    .iter()
                    .any(|c| c.deliveries.len() >= 16 || c.media_responses.len() >= 64);
            let projection = root.project(summary, repo, &self.keys).await;
            if projection.is_err() {
                root.detail_incomplete = true;
            }
            stored.version += 1;
            stored.blob = root.encode(&self.keys)?;
            let expected = stored.version - 1;
            if repo.compare_exchange(stored, expected).await? {
                return projection;
            }
            metrics::counter!("cmaeg_activity_history_summary_conflicts_total").increment(1);
        }
        Err(Error::Unavailable)
    }
}
