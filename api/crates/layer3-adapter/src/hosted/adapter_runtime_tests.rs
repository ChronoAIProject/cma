use super::*;

fn state(generation: u64) -> RuntimeState {
    RuntimeState {
        own_credential: false,
        home_directory: "/home/cma".into(),
        provision: ProviderIdentity {
            sandbox_id: "sandbox".into(),
            generation,
            image_sha256: "a".repeat(43),
            spec_fingerprint: "b".repeat(43),
            runtime_contract: "contract-3".into(),
        },
        home_volume: None,
    }
}

#[test]
fn cache_replacement_requires_same_sandbox_and_strictly_newer_generation() {
    let current = state(1);
    assert!(supersedes(&current, &state(2)));
    assert!(!supersedes(&current, &state(1)));
    assert!(!supersedes(&state(2), &state(1)));

    let mut foreign = state(2);
    foreign.provision.sandbox_id = "another-sandbox".into();
    assert!(!supersedes(&current, &foreign));

    let mut changed_home = state(2);
    changed_home.home_directory = "/home/other".into();
    assert!(!supersedes(&current, &changed_home));
}
