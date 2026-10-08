//! The seam between the signed-in pages and the console's channel memory.
//! [`super::memory::ConsoleMemory`] implements [`ChannelDirectory`] over its
//! owner index, summaries and activity rings; [`NoChannels`] answers an empty
//! page and no channel for a console composed without memory, such as the
//! sign-in tests'. Every read takes an [`Authorized`], so no implementation
//! can read channels for a bare owner string.
use std::sync::Arc;

use cmaeg_core::ids::{ChannelId, RecordId};
use futures_util::future::BoxFuture;

use super::{
    activity::{ActivityEntry, ActivityFilter, RecentActivity},
    authority::Authorized,
    producers::ProducerView,
    summary::ChannelSummary,
};

/// One page of the owner's channels, as the layout shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelPage {
    pub rows: Vec<ChannelRow>,
    /// Channels the owner has in all, across pages.
    pub total: usize,
    /// While the index is still loading: how many records it has seen.
    pub loading: Option<u64>,
    /// True when the index stopped at its bound and the list may be short.
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub record_id: String,
    /// Empty until the channel's summary has been read.
    pub name: String,
    /// The version the owner index holds.
    pub version: u64,
    /// The summary memory holds for that version, if any.
    pub summary: Option<Arc<ChannelSummary>>,
    /// The last hour of the channel's activity ring.
    pub activity: RecentActivity,
}

/// One channel `auth` owns, as its detail pages show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelView {
    pub record: RecordId,
    /// The version the owner index holds.
    pub version: u64,
    pub summary: Arc<ChannelSummary>,
    /// The channel id to display, when one was seen.
    pub channel_id: Option<ChannelId>,
}

pub trait ChannelDirectory: Send + Sync {
    /// The `page`-th page of `auth.owner()`'s channels, filtered by name.
    /// It may read CMA, under the console's own permits.
    fn channels_for<'a>(
        &'a self,
        auth: &'a Authorized,
        page: usize,
        query: Option<&'a str>,
    ) -> BoxFuture<'a, ChannelPage>;

    /// `record` when `auth` owns it; `None` for an unknown record or
    /// another owner's. It reads CMA only when no summary is cached for the
    /// indexed version.
    fn channel<'a>(
        &'a self,
        auth: &'a Authorized,
        record: &'a RecordId,
    ) -> BoxFuture<'a, Option<ChannelView>>;

    /// The newest activity entries of `record` matching `filter`, newest
    /// first, when `auth` owns it.
    fn activity(
        &self,
        auth: &Authorized,
        record: &RecordId,
        filter: ActivityFilter,
    ) -> Option<Vec<ActivityEntry>>;

    /// `record`'s custom producers, most recently seen first, when `auth`
    /// owns it.
    fn producers(&self, auth: &Authorized, record: &RecordId) -> Option<Vec<ProducerView>>;
}

/// No channel memory: every owner has an empty list.
pub struct NoChannels;

impl ChannelDirectory for NoChannels {
    fn channels_for<'a>(
        &'a self,
        _auth: &'a Authorized,
        _page: usize,
        _query: Option<&'a str>,
    ) -> BoxFuture<'a, ChannelPage> {
        Box::pin(std::future::ready(ChannelPage::default()))
    }

    fn channel<'a>(
        &'a self,
        _auth: &'a Authorized,
        _record: &'a RecordId,
    ) -> BoxFuture<'a, Option<ChannelView>> {
        Box::pin(std::future::ready(None))
    }

    fn activity(
        &self,
        _auth: &Authorized,
        _record: &RecordId,
        _filter: ActivityFilter,
    ) -> Option<Vec<ActivityEntry>> {
        None
    }

    fn producers(&self, _auth: &Authorized, _record: &RecordId) -> Option<Vec<ProducerView>> {
        None
    }
}
