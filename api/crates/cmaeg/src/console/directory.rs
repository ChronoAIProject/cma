//! The seam between the signed-in layout and the console's channel memory.
//! The console-memory work (#1001) implements [`ChannelDirectory`] over its
//! owner index; until then [`NoChannels`] answers an empty page. Every read
//! takes an [`Authorized`], so no implementation can list channels for a
//! bare owner string.
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
    fn channels_for(&self, auth: &Authorized, page: usize, query: Option<&str>) -> ChannelPage;
}

/// No channel memory yet: every owner has an empty list.
pub struct NoChannels;

impl ChannelDirectory for NoChannels {
    fn channels_for(&self, _auth: &Authorized, _page: usize, _query: Option<&str>) -> ChannelPage {
        ChannelPage::default()
    }
}
