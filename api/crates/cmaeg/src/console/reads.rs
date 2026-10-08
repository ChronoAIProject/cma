//! The console's owner-checked reads of its memory (Amendment R6 §R6.4).
//!
//! Every read takes the request's freshly verified [`Authorized`] and first
//! checks that the owner index gives the record to `auth.owner()`; summary
//! reads also compare the summary's own owner. Another owner's record reads
//! as `None`, which the pages answer with 404. No read takes a bare owner
//! string.
use std::{sync::Arc, time::Duration};

use cmaeg_core::ids::{ChannelId, RecordId};
use futures_util::{future::BoxFuture, FutureExt};

use super::{
    activity::{ActivityEntry, ActivityFilter, RingTotals},
    authority::Authorized,
    channel_ids::LinkError,
    directory::{ChannelDirectory, ChannelPage, ChannelRow},
    memory::ConsoleMemory,
    producers::ProducerView,
    summary::{created_channel_id, ChannelSummary, SummaryStatus},
};
use crate::channels::management::ManagementError;

/// Channels on one page of the console's list.
pub const PAGE_ROWS: usize = 50;
/// Activity entries one read returns.
pub const ACTIVITY_ROWS: usize = 50;
/// How long a page waits for its owner's re-listing and its summaries
/// before it renders what memory holds.
const PAGE_WAIT: Duration = Duration::from_secs(3);

impl ConsoleMemory {
    /// `record`'s summary when `auth` owns it. A record the index has no
    /// head for is read once with the owner check and then indexed.
    pub async fn summary(
        &self,
        auth: &Authorized,
        record: &RecordId,
    ) -> Option<Arc<ChannelSummary>> {
        let Some(version) = self.index.version_for(auth, record) else {
            return self.adopt(auth, record).await;
        };
        match self.summaries.get(record, version) {
            Some(summary) => owned(summary, auth),
            None => self.fill(auth, record).await,
        }
    }

    /// The newest [`ACTIVITY_ROWS`] activity entries of `record` matching
    /// `filter`, newest first, when `auth` owns it.
    pub fn activity(
        &self,
        auth: &Authorized,
        record: &RecordId,
        filter: ActivityFilter,
    ) -> Option<Vec<ActivityEntry>> {
        self.index.version_for(auth, record)?;
        Some(self.rings.entries(record, &filter, ACTIVITY_ROWS))
    }

    /// The activity entry of `record`'s event `event_key`, when `auth` owns
    /// the record and the entry is still held.
    pub fn event(
        &self,
        auth: &Authorized,
        record: &RecordId,
        event_key: &str,
    ) -> Option<ActivityEntry> {
        self.index.version_for(auth, record)?;
        self.rings.event(record, event_key)
    }

    /// `record`'s custom producers, most recently seen first, when `auth`
    /// owns it.
    pub fn producers(&self, auth: &Authorized, record: &RecordId) -> Option<Vec<ProducerView>> {
        self.index.version_for(auth, record)?;
        Some(self.rings.producers(record))
    }

    /// Totals across every channel's activity, which name no channel or
    /// owner: for the operator's gauges and tests.
    pub fn activity_totals(&self) -> RingTotals {
        self.rings.totals()
    }

    /// The channel id to display for `record`, when `auth` owns it and the
    /// id has been seen.
    pub fn channel_id(&self, auth: &Authorized, record: &RecordId) -> Option<ChannelId> {
        self.index.version_for(auth, record)?;
        self.channel_ids.get(record)
    }

    /// Remember `id` as `record`'s channel id when `auth` owns the record and
    /// the id opens to it. The pointer is checked, never re-sealed.
    pub async fn link(
        &self,
        auth: &Authorized,
        record: &RecordId,
        id: &ChannelId,
    ) -> Result<(), LinkError> {
        if self.index.version_for(auth, record).is_none()
            && self.adopt(auth, record).await.is_none()
        {
            return Err(LinkError::NotFound);
        }
        self.reader.opens_to(id, record)?;
        self.channel_ids.learn(record.clone(), id.clone());
        Ok(())
    }

    /// One page of `auth`'s channels, optionally only those whose name
    /// contains `query`. The owner is re-listed from CMA at most every 30 s,
    /// and at most [`PAGE_ROWS`] missing summaries are read per page.
    pub async fn channels_for(
        &self,
        auth: &Authorized,
        page: usize,
        query: Option<&str>,
    ) -> ChannelPage {
        // A slow CMA leaves the page with what memory already holds.
        if tokio::time::timeout(PAGE_WAIT, self.reconcile(auth))
            .await
            .is_err()
        {
            tracing::debug!("console owner re-listing outlasted the page wait");
        }
        let records = self.index.records_for(auth);
        let needle = query
            .map(str::to_lowercase)
            .filter(|value| !value.is_empty());
        let wanted: Vec<RecordId> = match &needle {
            // Names are known only from summaries: read the first missing
            // ones, then match every summary memory holds.
            Some(_) => records
                .iter()
                .filter(|(record, version)| self.summaries.get(record, *version).is_none())
                .take(PAGE_ROWS)
                .map(|(record, _)| record.clone())
                .collect(),
            None => records
                .iter()
                .skip(page.saturating_mul(PAGE_ROWS))
                .take(PAGE_ROWS)
                .map(|(record, _)| record.clone())
                .collect(),
        };
        let fills =
            futures_util::future::join_all(wanted.iter().map(|record| self.summary(auth, record)));
        if tokio::time::timeout(PAGE_WAIT, fills).await.is_err() {
            tracing::debug!("console summaries outlasted the page wait; names may be missing");
        }
        let named: Vec<ChannelRow> = records
            .iter()
            .map(|(record, version)| ChannelRow {
                record_id: record.to_string(),
                name: self
                    .summaries
                    .get(record, *version)
                    .and_then(|summary| owned(summary, auth))
                    .map(|summary| summary.name.clone())
                    .unwrap_or_default(),
            })
            .filter(|row| {
                needle
                    .as_deref()
                    .is_none_or(|needle| row.name.to_lowercase().contains(needle))
            })
            .collect();
        let status = self.index_status();
        ChannelPage {
            total: named.len(),
            rows: named
                .into_iter()
                .skip(page.saturating_mul(PAGE_ROWS))
                .take(PAGE_ROWS)
                .collect(),
            loading: status.loading,
            partial: status.partial,
        }
    }

    /// Read `record` for `auth` under a console permit and cache its
    /// summary at the version read.
    async fn fill(&self, auth: &Authorized, record: &RecordId) -> Option<Arc<ChannelSummary>> {
        let read = {
            let _permit = self.budget.permit().await;
            self.reader.read(auth.creator(), record).await
        };
        match read {
            Ok(recorded) => {
                if let Some(id) = created_channel_id(&recorded.definition) {
                    self.channel_ids.learn(record.clone(), id);
                }
                let summary = Arc::new(ChannelSummary::of(
                    &recorded.definition,
                    recorded.record_bytes,
                    recorded.valid,
                ));
                self.index.upsert(
                    record.clone(),
                    recorded.definition.owner.subject.clone(),
                    recorded.version,
                );
                self.summaries
                    .insert(record.clone(), recorded.version, summary.clone());
                owned(summary, auth)
            }
            Err(ManagementError::NotFound) => {
                // Deleted since it was indexed for this owner.
                if self.index.version_for(auth, record).is_some() {
                    self.index.remove(record);
                }
                self.misses
                    .insert((auth.owner().to_owned(), record.clone()), ());
                None
            }
            Err(ManagementError::Internal(_)) => {
                // The head is the owner's; only its definition is unreadable.
                let version = self.index.version_for(auth, record)?;
                let summary = Arc::new(ChannelSummary::without_definition(
                    auth.owner(),
                    SummaryStatus::Unreadable,
                ));
                self.summaries
                    .insert(record.clone(), version, summary.clone());
                Some(summary)
            }
            Err(error) => {
                tracing::warn!(%error, "console summary read failed");
                self.index.version_for(auth, record)?;
                Some(Arc::new(ChannelSummary::without_definition(
                    auth.owner(),
                    SummaryStatus::Unavailable,
                )))
            }
        }
    }

    /// A record the index has no head for: read it once for `auth`, and
    /// index it when it is theirs. A recent miss is not read again.
    async fn adopt(&self, auth: &Authorized, record: &RecordId) -> Option<Arc<ChannelSummary>> {
        let key = (auth.owner().to_owned(), record.clone());
        if self.misses.contains_key(&key) {
            return None;
        }
        let summary = self.fill(auth, record).await;
        summary.filter(|summary| summary.status != SummaryStatus::Unavailable)
    }
}

/// `summary` when its own owner is `auth`'s.
fn owned(summary: Arc<ChannelSummary>, auth: &Authorized) -> Option<Arc<ChannelSummary>> {
    (summary.owner == auth.owner()).then_some(summary)
}

impl ChannelDirectory for ConsoleMemory {
    fn channels_for<'a>(
        &'a self,
        auth: &'a Authorized,
        page: usize,
        query: Option<&'a str>,
    ) -> BoxFuture<'a, ChannelPage> {
        ConsoleMemory::channels_for(self, auth, page, query).boxed()
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod read_tests;
