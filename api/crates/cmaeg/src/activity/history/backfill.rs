//! Imports only surviving bounded snapshots, without replaying ingress/provider work.
use super::{payload::Root, repository::*, ActivityHistory, Error};
use crate::activity::{Change, ChannelActivity};

impl ActivityHistory {
    pub(crate) async fn import_legacy(
        &self,
        owner: &str,
        record: &str,
        mut legacy: ChannelActivity,
        original_expiry_ms: i64,
    ) -> Result<(), Error> {
        let repo = self.reader()?;
        let received_ms = time::OffsetDateTime::parse(
            &legacy.received_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| Error::Unreadable)?
        .unix_timestamp_nanos()
            / 1_000_000;
        let received_ms = i64::try_from(received_ms).map_err(|_| Error::Unreadable)?;
        let expires_ms = original_expiry_ms.min(
            received_ms
                .checked_add(self.config.retention_ms())
                .ok_or(Error::Unreadable)?,
        );
        if expires_ms <= super::now_ms() {
            return Ok(());
        }
        legacy.received_at = super::paging::format_time(received_ms)?;
        let scope = super::scope(owner, record);
        let key = super::event_key(&scope, &legacy.event_id);
        let mut receipt = legacy.clone();
        receipt.consumers.clear();
        let root = Root {
            schema: 1,
            scope: scope.clone(),
            event_key: key.clone(),
            received_ms,
            expires_ms,
            receipt: receipt.clone(),
            inline: Some(receipt),
            manifest: None,
            legacy_partial: true,
            detail_incomplete: false,
            detail_truncated: false,
        };
        let mut stored = repo
            .insert(StoredEvent {
                scope: scope.clone(),
                key: Position {
                    received_ms,
                    event_key: key.clone(),
                },
                version: 1,
                sequence: 0,
                expires_ms,
                blob: root.encode(&self.keys)?,
            })
            .await?;
        for _ in 0..8 {
            let mut root = Root::decode(&self.keys, &stored)?;
            let mut summary = root.summary(repo, &self.keys, super::now_ms()).await?;
            let before = serde_json::to_vec(&summary).map_err(|_| Error::Unreadable)?;
            merge(&mut summary, &legacy);
            if serde_json::to_vec(&summary).map_err(|_| Error::Unreadable)? == before {
                return Ok(());
            }
            root.detail_truncated |= summary.consumers.len() >= 16
                || summary
                    .consumers
                    .iter()
                    .any(|c| c.deliveries.len() >= 16 || c.media_responses.len() >= 64);
            let projected = root.project(summary, repo, &self.keys).await;
            if projected.is_err() {
                root.detail_incomplete = true;
            }
            stored.version += 1;
            stored.blob = root.encode(&self.keys)?;
            let expected = stored.version - 1;
            if repo.compare_exchange(stored, expected).await? {
                return projected;
            }
            stored = repo
                .get(&scope, &key, super::now_ms())
                .await?
                .ok_or(Error::Expired)?;
        }
        Err(Error::Unavailable)
    }
    pub(crate) fn capturing(&self) -> bool {
        self.config.capture
    }
}
fn merge(current: &mut ChannelActivity, legacy: &ChannelActivity) {
    for old in &legacy.consumers {
        let existing = current
            .consumers
            .iter()
            .find(|c| c.consumer_id == old.consumer_id)
            .cloned();
        let Some(existing) = existing else {
            if current.consumers.len() < 16 {
                current.consumers.push(old.clone());
            }
            continue;
        };
        let mut change = Change::state(
            &old.consumer_id,
            old.state.clone(),
            existing
                .response_id
                .clone()
                .or_else(|| old.response_id.clone()),
        );
        // Merge through the existing monotonic state rules. Presentation and
        // immutable timestamps from the live summary win when already present.
        change.apply(current);
        let consumer = current
            .consumers
            .iter_mut()
            .find(|c| c.consumer_id == old.consumer_id)
            .unwrap();
        macro_rules! field {
            ($name:ident) => {
                consumer.$name = existing.$name.clone().or_else(|| old.$name.clone());
            };
        }
        field!(refusal_code);
        field!(last_non_admission_code);
        field!(display_name);
        field!(agent_id);
        field!(provider);
        field!(conversation_id);
        field!(dispatched_at);
        field!(accepted_at);
        field!(produced_at);
        field!(completed_at);
        for response in &old.media_responses {
            if consumer.media_responses.len() < 64
                && !consumer
                    .media_responses
                    .iter()
                    .any(|r| r.response_id == response.response_id)
            {
                consumer.media_responses.push(response.clone());
            }
        }
        let additions: Vec<_> = old
            .deliveries
            .iter()
            .filter(|old| {
                !consumer.deliveries.iter().any(|new| {
                    new.step == old.step
                        && new.destination == old.destination
                        && new.recipients == old.recipients
                        && new.platform_message_id == old.platform_message_id
                })
            })
            .cloned()
            .collect();
        for delivery in additions {
            change.state = None;
            change.delivery = Some(delivery);
            change.apply(current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{
        history::tests::{query, row, setup},
        ConsumerState,
    };
    #[tokio::test]
    async fn repeated_backfill_preserves_original_expiry_merges_live_facts_and_stays_partial() {
        let (memory, service, at) = setup();
        let expiry = at + 2 * 86_400_000;
        let mut legacy = row("legacy", at);
        Change::state("old", ConsumerState::Completed, None).apply(&mut legacy);
        service
            .import_legacy("owner", "channel", legacy.clone(), expiry)
            .await
            .unwrap();
        service
            .change(
                "owner",
                "channel",
                "legacy",
                &Change::state("live", ConsumerState::Completed, None),
            )
            .await
            .unwrap();
        service
            .import_legacy("owner", "channel", legacy, expiry)
            .await
            .unwrap();
        assert_eq!(memory.events.lock().unwrap().len(), 1);
        assert_eq!(
            memory
                .events
                .lock()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .expires_ms,
            expiry
        );
        let page = service
            .page("owner", "channel", query(at, 100))
            .await
            .unwrap();
        assert_eq!(page.items[0].activity.consumers.len(), 2);
        assert!(page.items[0].legacy_partial);
        assert!(!page.coverage.complete_event_enumeration);
    }
}
