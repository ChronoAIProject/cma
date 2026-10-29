//! Owner-visible recent channel activity. Encrypted snapshots are independent
//! of execution authority; observing progress never changes a provider turn.
pub(crate) mod facts;
mod model;
mod observer;
mod store;
pub use model::{
    ActivityConsumer, ActivityDelivery, ActivityMediaResponse, ActivityPage, ActivityRecipient,
    ChannelActivity,
};
pub use model::{Change, ConsumerState};
pub use observer::ObservedActivity;
pub use store::ActivityStore;

pub(crate) fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp")
}
