//! The seam between the signed-in layout and the console's channel memory.
//! [`super::memory::ConsoleMemory`] implements [`ChannelDirectory`] over its
//! owner index and summaries; [`NoChannels`] answers an empty page for a
//! console composed without memory, such as the sign-in tests'. Every read
//! takes an [`Authorized`], so no implementation can list channels for a
//! bare owner string.
use futures_util::future::BoxFuture;

use super::authority::Authorized;

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
    pub name: String,
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
}
