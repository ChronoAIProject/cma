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
/// The names are the bridge's own — it offers `default`, `acceptEdits`,
/// `plan`, `dontAsk` and `bypassPermissions` — and only three of them
/// correspond to something the core can express. `plan` and `dontAsk` are
/// deliberately unreachable: neither is a level of authority, and mapping a
/// level onto one would answer a question about permission with a change of
/// working style.
pub(super) fn mode_for(permission: RuntimePermission) -> &'static str {
    match permission {
        // Asks before it acts, which is what a guarded session means.
        RuntimePermission::Guarded => "default",
        // Edits without asking, still stopping for anything beyond the
        // workspace.
        RuntimePermission::Autonomous => "acceptEdits",
        // No approvals and no confinement inside the sandbox. Survivable only
        // because the sandbox is still the boundary.
        //
        // The bridge withholds this mode entirely when it is running as root,
        // so it depends on the runtime image keeping its unprivileged user —
        // which the manifest policy gate requires anyway. Should that ever
        // change, this mode stops being offered and a session asking for it is
        // refused rather than quietly downgraded, because the shared client
        // fails a handshake whose mode did not take.
        RuntimePermission::Unrestricted => "bypassPermissions",
    }
}

#[cfg(test)]
#[path = "permission_tests.rs"]
mod permission_tests;
