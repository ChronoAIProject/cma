use super::*;

fn request() -> ProviderSandboxRequest {
    ProviderSandboxRequest {
        image: ProviderImage {
            uri: "registry.example/runtime@sha256:abc".into(),
        },
        entrypoint: vec!["/runtime".into(), "serve".into()],
        env: BTreeMap::from([
            ("HOME".into(), "/home/cma".into()),
            ("TERM".into(), "xterm-256color".into()),
        ]),
        resource_limits: BTreeMap::from([
            ("cpu".into(), "1000m".into()),
            ("memory".into(), "1024Mi".into()),
        ]),
        timeout: Some(3_600),
        metadata: BTreeMap::from([
            (SANDBOX_METADATA.into(), "sandbox-1".into()),
            ("cma/runtime-contract".into(), "2".into()),
            (CONTROL_PLANE_METADATA.into(), "rollout-a".into()),
        ]),
    }
}

#[test]
fn provider_fingerprint_has_a_stable_golden_encoding() {
    assert_eq!(
        provider_request_fingerprint(&request()).unwrap(),
        "vhTpfKGCsjfSjaATszKeZPGILJ9GDZqjjDNnDtxLpi0"
    );
}

#[test]
fn provider_fingerprint_covers_every_stable_request_field() {
    let original = request();
    let expected = provider_request_fingerprint(&original).unwrap();
    let mut variants = Vec::new();

    let mut changed = original.clone();
    changed.image.uri.push_str("-changed");
    variants.push(changed);
    let mut changed = original.clone();
    changed.entrypoint.push("--changed".into());
    variants.push(changed);
    let mut changed = original.clone();
    changed.env.insert("EXTRA".into(), "changed".into());
    variants.push(changed);
    let mut changed = original.clone();
    changed.resource_limits.insert("cpu".into(), "2000m".into());
    variants.push(changed);
    let mut changed = original.clone();
    changed.timeout = None;
    variants.push(changed);
    let mut changed = original.clone();
    changed
        .metadata
        .insert("cma/runtime-contract".into(), "changed".into());
    variants.push(changed);

    assert!(variants
        .iter()
        .all(|changed| provider_request_fingerprint(changed).unwrap() != expected));
}

#[test]
fn provider_fingerprint_ignores_observational_control_plane_metadata() {
    let original = request();
    let expected = provider_request_fingerprint(&original).unwrap();
    let mut changed = original;
    changed
        .metadata
        .insert(CONTROL_PLANE_METADATA.into(), "rollout-b".into());

    assert_eq!(provider_request_fingerprint(&changed).unwrap(), expected);
}

#[test]
fn provider_fingerprint_cannot_include_itself() {
    let mut recursive = request();
    recursive
        .metadata
        .insert(SPEC_FINGERPRINT_METADATA.into(), "recursive".into());
    assert!(provider_request_fingerprint(&recursive).is_err());
}
