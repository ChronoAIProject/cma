//! Provider-neutral home backup scheduling and catalog records.

mod deletion;
mod erasure;
mod model;
mod replay;
pub mod retention;
mod store;
pub use deletion::*;
pub use erasure::{erase_home_backups, HomeBackupErasureError, HomeBackupErasureService};
pub use model::*;
pub use replay::{reject_deleted_owner, DeletionReplayError, DeletionReplayPort};
pub use store::*;

#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod tests;
