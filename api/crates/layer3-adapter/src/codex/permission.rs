//! This kernel's name for how much its agent may do unattended.
//!
//! Its own file, and its own directory, because it is the one place a kernel's
//! private vocabulary decides how much authority a session carries. The shared
//! client sends whatever name it is handed and cannot check it; getting this
//! wrong grants more than somebody asked for, and nothing downstream would say
//! so.

use crate::RuntimePermission;

/// What this bridge calls each level of authority.
///
/// The bridge exposes modes rather than the approval policy the kernel's own
/// configuration file uses, and it starts every session in `agent` regardless
/// of that file — which is why this is sent explicitly and why failing to send
/// it fails the session.
pub(super) fn mode_for(
    permission: RuntimePermission,
    restricted_service_gateway: bool,
) -> &'static str {
    match permission {
        RuntimePermission::Guarded => "read-only",
        RuntimePermission::Autonomous if restricted_service_gateway => "cma-trigger",
        RuntimePermission::Autonomous => "agent",
        // No approvals and no confinement inside the sandbox. Survivable only
        // because the sandbox is still the boundary.
        RuntimePermission::Unrestricted => "agent-full-access",
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod permission_tests;
