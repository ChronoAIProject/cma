//! Owner-visible recent channel activity. Encrypted snapshots are independent
//! of execution authority; observing progress never changes a provider turn.
mod execution;
pub(crate) mod facts;
pub mod history;
mod model;
mod observer;
pub(crate) mod public_projection;
mod store;
pub(crate) use execution::ExecutionAttribution;
pub use model::{
    ActivityConsumer, ActivityDelivery, ActivityMediaResponse, ActivityPage, ActivityRecipient,
    ChannelActivity,
};
pub use model::{Change, ConsumerState};
pub use observer::ObservedActivity;
pub(crate) use public_projection::public_page;
pub use store::ActivityStore;

pub(crate) fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamp")
}
