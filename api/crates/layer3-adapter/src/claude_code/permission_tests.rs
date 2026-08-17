//! What this kernel is told about how much it may do unattended.

use super::mode_for;
use crate::RuntimePermission;

#[test]
fn each_level_of_authority_has_this_bridges_own_name_for_it() {
    assert_eq!(mode_for(RuntimePermission::Guarded), "default");
    assert_eq!(mode_for(RuntimePermission::Autonomous), "acceptEdits");
    assert_eq!(
        mode_for(RuntimePermission::Unrestricted),
        "bypassPermissions"
    );
}

#[test]
fn a_guarded_session_is_never_started_in_a_mode_that_acts_alone() {
    // The whole point of the level. This is the assertion that fails if
    // somebody renames a mode without reading what it grants — the bridge
    // accepts any name it knows and says nothing about which of them ask.
    let guarded = mode_for(RuntimePermission::Guarded);
    assert_ne!(guarded, "acceptEdits");
    assert_ne!(guarded, "bypassPermissions");
    assert_ne!(guarded, "dontAsk");
}
