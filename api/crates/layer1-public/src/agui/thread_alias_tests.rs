//! Semantic creation-alias fingerprint checks.

use super::*;

#[test]
fn only_creation_affecting_properties_distinguish_an_alias() {
    let absent = admission::fingerprint(&CreationFingerprint {
        environment_profile: None,
    })
    .unwrap();
    let same = admission::fingerprint(&CreationFingerprint {
        environment_profile: None,
    })
    .unwrap();
    let profiled = admission::fingerprint(&CreationFingerprint {
        environment_profile: Some("envp_contract"),
    })
    .unwrap();

    assert_eq!(absent, same);
    assert_ne!(absent, profiled);
}
