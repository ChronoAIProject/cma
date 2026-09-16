//! Actual old/new binaries share only owned durable fixtures and nonsecret identities.
use super::*;
use mongodb::bson::doc;
use serde::{Deserialize, Serialize};

#[path = "compatibility/capacity.rs"]
mod capacity;
#[path = "compatibility/prepare.rs"]
mod prepare;
#[path = "compatibility/protocol.rs"]
mod protocol;
#[path = "compatibility/resume.rs"]
mod resume;
#[path = "compatibility/signed.rs"]
mod signed;
#[path = "compatibility/worker.rs"]
mod worker;
use protocol::Harness;

pub(crate) async fn channel_control_compatibility_prepare_actual_records() {
    prepare::scenario().await;
}

pub(crate) async fn channel_control_compatibility_compatible_capacity_counts_held_control() {
    capacity::exercise(&Harness::open().await, true).await;
}

pub(crate) async fn channel_control_compatibility_rollback_resumes_retained_control_with_fresh_admission_disabled(
) {
    resume::exercise(&Harness::open().await).await;
}
