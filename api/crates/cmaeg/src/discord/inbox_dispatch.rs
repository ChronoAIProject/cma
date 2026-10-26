//! Native work enters only the durable routed admission path; never direct execution.
use super::{
    inbox_scan::{Disposition, Entry, InboxStore},
    runtime_source, GatewayError, InboxLaunch, InboxReader,
};
use crate::{
    ingress::{launch::LaunchOutcome, IngressState},
    records::RecordClient,
};
use cmaeg_core::seal::MasterKeyRing;
use std::{collections::BTreeSet, sync::Arc};

pub struct InboxDispatcher {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    ingress: IngressState,
    selected: super::selection::Selection,
    reader: InboxReader,
    store: InboxStore,
}
pub struct DispatchPage {
    pub next_cursor: Option<String>,
    pub acknowledged: usize,
    pub pending: usize,
    pub invalid: usize,
}
impl InboxDispatcher {
    pub fn new(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        ingress: IngressState,
        selected: BTreeSet<String>,
    ) -> Self {
        Self::with_selection(
            records,
            keys,
            ingress,
            super::selection::Selection::new(&crate::NativeDiscordConfig {
                enabled: true,
                discovery_enabled: false,
                records: selected,
            }),
        )
    }
    pub(crate) fn with_selection(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        ingress: IngressState,
        selected: super::selection::Selection,
    ) -> Self {
        Self {
            reader: InboxReader::new(records.clone(), keys.clone()),
            store: InboxStore::new(records.clone(), keys.clone()),
            records,
            keys,
            ingress,
            selected,
        }
    }
    pub async fn page(&self, after: Option<&str>, now: i64) -> Result<DispatchPage, GatewayError> {
        if self.selected.is_empty() {
            return Ok(DispatchPage {
                next_cursor: None,
                acknowledged: 0,
                pending: 0,
                invalid: 0,
            });
        }
        let page = self.store.page(after, now).await?;
        let mut report = DispatchPage {
            next_cursor: page.next_cursor,
            acknowledged: 0,
            pending: 0,
            invalid: page.invalid,
        };
        for entry in page.entries {
            if !self.selected.contains(&entry.inbox.channel_record) {
                continue;
            }
            match self.dispatch(&entry, now).await {
                Ok(true) => report.acknowledged += 1,
                Ok(false)
                | Err(
                    GatewayError::Unavailable | GatewayError::RateLimited | GatewayError::Authority,
                ) => report.pending += 1,
                Err(_) => report.invalid += 1,
            }
        }
        Ok(report)
    }
    async fn dispatch(&self, entry: &Entry, now: i64) -> Result<bool, GatewayError> {
        if self.store.settled(entry).await? {
            return Ok(true);
        }
        let current = runtime_source::resolve(
            &self.records,
            &self.keys,
            &self.ingress.resolver,
            &entry.inbox.channel_record,
        )
        .await?;
        if current.channel.definition.owner.subject != entry.owner
            || current.revision != entry.inbox.source_revision
        {
            return Err(GatewayError::Authority);
        }
        // Repair a crash between inbox commit and Activity persistence before
        // checking launch capacity or expiring the original reply authority.
        self.reader
            .observe(
                &entry.id,
                &current.channel,
                now,
                self.ingress.activity.as_ref(),
            )
            .await?;
        let disposition = if entry.expired(now) {
            Disposition::Expired
        } else {
            match self.reader.load(&entry.id, current.channel, now).await? {
                InboxLaunch::Refused(reason) => Disposition::Refused {
                    reason: serde_json::to_value(reason)
                        .map_err(|_| GatewayError::Protocol)?
                        .as_str()
                        .ok_or(GatewayError::Protocol)?
                        .into(),
                },
                InboxLaunch::Launch(verified) => {
                    let permit = self
                        .ingress
                        .inflight
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| GatewayError::Unavailable)?;
                    match self.ingress.launch_native_verified(*verified, permit).await {
                        LaunchOutcome::Admitted { admission_id, .. } => {
                            Disposition::Admitted { admission_id }
                        }
                        LaunchOutcome::Refused(problem)
                            if matches!(problem.status, 400 | 403 | 409 | 410 | 413 | 422) =>
                        {
                            Disposition::Refused {
                                reason: format!("admission_refused_{}", problem.status),
                            }
                        }
                        LaunchOutcome::Refused(_) => return Ok(false),
                        LaunchOutcome::Accepted { .. } | LaunchOutcome::Deferred => {
                            return Err(GatewayError::Protocol)
                        }
                    }
                }
            }
        };
        // A lost acknowledgement is read back as the exact immutable value.
        // If it is absent, retry reopens the original routed admission safely.
        self.store.acknowledge(entry, disposition).await?;
        Ok(true)
    }
}
