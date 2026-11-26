//! Authenticated receiver messages. External handles are adapter assertions;
//! the receiver separately checks its locally assigned Sandbox and boot.
use super::DesktopOwnership;
use serde::{Deserialize, Serialize};

pub const DESKTOP_OWNER_HEADER: &str = "x-cma-desktop-owner";
// The WebSocket client can receive the headers before the error body arrives.
pub const DESKTOP_STATE_HEADER: &str = "x-cma-desktop-state";
pub const DESKTOP_STATE_FAILED: &str = "failed";
pub const DESKTOP_RECEIVER_LEASE_MS: u64 = 10_000;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopChallengeRequest {
    pub sandbox_id: String,
    pub runtime_id: String,
    /// None starts one new connection; Some renews this exact live session.
    pub previous: Option<DesktopOwnership>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopConfirmation {
    pub ownership: DesktopOwnership,
    pub nonce: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopAcknowledgement {
    pub ownership: DesktopOwnership,
}
