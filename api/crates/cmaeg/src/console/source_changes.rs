//! The seam through which the console changes a channel's sources.
//!
//! The console never writes a channel itself: attach and detach go through
//! the same record-keyed management functions as REST, with the creator of
//! the request's `Authorized`. Those functions arrive with
//! `CMAEG_DYNAMIC_SOURCES_ENABLED` (#999); until then the gateway composes
//! [`PendingSourceChanges`], so the pages say plainly that source changes
//! are not available yet, and a direct post is refused without any write.
use cmaeg_core::{
    channel::{PartitionKey, Secret, SourceId},
    ids::RecordId,
};
use futures_util::future::BoxFuture;

use crate::{
    auth::Creator,
    channels::{HmacSecretRequest, SourceRequest},
};

/// Whether the console may offer source changes now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAvailability {
    /// Attach and detach are offered.
    Available,
    /// The operator has `CMAEG_DYNAMIC_SOURCES_ENABLED` off.
    Disabled,
    /// This image has no record-keyed source mutations yet.
    NotYetAvailable,
}

/// An attach, shaped as management's source attach request: the source
/// exactly as REST takes it, the secrets it brings, and the version the
/// owner saw.
#[derive(Debug)]
pub struct AttachRequest {
    pub expected_version: u64,
    pub source: SourceRequest,
    pub credentials: AttachCredentials,
    /// Required for a custom source on a chat-keyed channel.
    pub partition_custom_key: Option<PartitionKey>,
}

/// The secrets an attach brings. `Secret` never prints its value.
#[derive(Debug, Default)]
pub struct AttachCredentials {
    pub channel_key: Option<Secret>,
    pub telegram_bot_token: Option<Secret>,
    pub telegram_webhook_secret: Option<Secret>,
    pub hmac_secrets: Vec<HmacSecretRequest>,
}

/// A completed attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    /// The attached source's slot.
    pub source_id: SourceId,
    /// The channel's version after the write.
    pub version: u64,
}

/// Why a source change was not made. Every refusal is made before any
/// write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceChangeError {
    NotYetAvailable,
    Disabled,
    /// Management refused the request at `pointer` with a fixed message.
    Invalid {
        pointer: String,
        message: String,
    },
    /// The channel changed since the owner's page was loaded.
    Conflict,
    /// The source is the channel's default reply source.
    DefaultReplyInUse,
    /// No such channel or source for this owner.
    NotFound,
    /// CMA or the gateway could not complete the change now.
    Unavailable,
}

pub trait SourceChanges: Send + Sync {
    fn availability(&self) -> SourceAvailability;

    /// Attach `request`'s source to `record` for `creator`.
    fn attach<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
        request: AttachRequest,
    ) -> BoxFuture<'a, Result<Attached, SourceChangeError>>;

    /// Detach `source_id` from `record` for `creator`; the channel's
    /// version after the write.
    fn detach<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
        source_id: &'a SourceId,
        expected_version: u64,
    ) -> BoxFuture<'a, Result<u64, SourceChangeError>>;
}

/// The composition until record-keyed source mutations exist: nothing is
/// offered and nothing is written.
pub struct PendingSourceChanges;

impl SourceChanges for PendingSourceChanges {
    fn availability(&self) -> SourceAvailability {
        SourceAvailability::NotYetAvailable
    }

    fn attach<'a>(
        &'a self,
        _creator: &'a Creator,
        _record: &'a RecordId,
        _request: AttachRequest,
    ) -> BoxFuture<'a, Result<Attached, SourceChangeError>> {
        Box::pin(std::future::ready(Err(SourceChangeError::NotYetAvailable)))
    }

    fn detach<'a>(
        &'a self,
        _creator: &'a Creator,
        _record: &'a RecordId,
        _source_id: &'a SourceId,
        _expected_version: u64,
    ) -> BoxFuture<'a, Result<u64, SourceChangeError>> {
        Box::pin(std::future::ready(Err(SourceChangeError::NotYetAvailable)))
    }
}
