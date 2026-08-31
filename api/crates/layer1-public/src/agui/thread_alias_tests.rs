//! Semantic creation-alias fingerprint checks.

use super::*;

#[test]
fn only_creation_affecting_properties_distinguish_an_alias() {
    let absent = admission::fingerprint(&CreationFingerprint {
        environment_profile: None,
        surface: None,
    })
    .unwrap();
    let same = admission::fingerprint(&CreationFingerprint {
        environment_profile: None,
        surface: None,
    })
    .unwrap();
    let profiled = admission::fingerprint(&CreationFingerprint {
        environment_profile: Some("envp_contract"),
        surface: None,
    })
    .unwrap();
    let chat = admission::fingerprint(&CreationFingerprint {
        environment_profile: None,
        surface: Some(layer2_core::SessionSurface::Chat),
    })
    .unwrap();

    assert_eq!(absent, same);
    assert_ne!(absent, profiled);
    assert_ne!(absent, chat);
    assert_ne!(profiled, chat);
}
