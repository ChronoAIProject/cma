//! How the console reads one channel record: through management's
//! record-keyed load, with REST's owner check, or a fake in tests.
use cmaeg_core::ids::{ChannelId, RecordId};
use futures_util::future::BoxFuture;

use super::channel_ids::{opens_to, LinkError};
use crate::{
    auth::Creator,
    channels::management::{load_by_record, ManagementError, ManagementState, RecordedChannel},
};

pub trait ChannelReader: Send + Sync {
    /// The channel `record` stores, if `creator` owns it.
    fn read<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
    ) -> BoxFuture<'a, Result<RecordedChannel, ManagementError>>;

    /// Whether `id` opens to `record` in this gateway's CMA.
    fn opens_to(&self, id: &ChannelId, record: &RecordId) -> Result<(), LinkError>;
}

impl ChannelReader for ManagementState {
    fn read<'a>(
        &'a self,
        creator: &'a Creator,
        record: &'a RecordId,
    ) -> BoxFuture<'a, Result<RecordedChannel, ManagementError>> {
        Box::pin(load_by_record(self, creator, record))
    }

    fn opens_to(&self, id: &ChannelId, record: &RecordId) -> Result<(), LinkError> {
        opens_to(id, record, &self.master_keys, &self.records)
    }
}
