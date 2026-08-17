//! What this kernel is told about how much it may do unattended.

use super::mode_for;
use crate::RuntimePermission;

#[test]
fn each_level_of_authority_has_this_bridges_own_name_for_it() {
    assert_eq!(mode_for(RuntimePermission::Guarded), "read-only");
    assert_eq!(mode_for(RuntimePermission::Autonomous), "agent");
    assert_eq!(
        mode_for(RuntimePermission::Unrestricted),
        "agent-full-access"
    );
}

#[test]
fn the_guarded_mode_is_the_one_that_cannot_act_alone() {
    // The whole point of the level: a session whose owner asked to be asked
    // must not be started in a mode that proceeds without asking. This is the
    // assertion that fails if somebody renames a mode without reading what it
    // grants.
    assert_eq!(
        mode_for(RuntimePermission::Guarded),
        "read-only",
        "a guarded session must not be started in a mode that can act unattended"
    );
}
