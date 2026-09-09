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
        volumes: Vec::new(),
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
fn persistent_mount_identity_cannot_be_replayed_with_another_claim_or_path() {
    use crate::provider::{ProviderPersistentVolumeClaim, ProviderVolume};

    let mut original = request();
    let without_volume = provider_request_fingerprint(&original).unwrap();
    original.volumes.push(ProviderVolume {
        name: "home".into(),
        pvc: ProviderPersistentVolumeClaim {
            claim_name: "home-one".into(),
            create_if_not_exists: false,
            delete_on_sandbox_termination: false,
        },
        mount_path: "/home/cma".into(),
        read_only: false,
        sub_path: Some("home".into()),
    });
    let expected = provider_request_fingerprint(&original).unwrap();
    assert_ne!(expected, without_volume);
    for field in 0..6 {
        let mut changed = original.clone();
        let volume = &mut changed.volumes[0];
        match field {
            0 => volume.pvc.claim_name = "home-two".into(),
            1 => volume.name = "different".into(),
            2 => volume.mount_path = "/different".into(),
            3 => volume.read_only = true,
            4 => volume.sub_path = Some("other".into()),
            _ => volume.sub_path = None,
        }
        assert_ne!(provider_request_fingerprint(&changed).unwrap(), expected);
    }
    original.volumes[0].pvc.create_if_not_exists = true;
    assert!(provider_request_fingerprint(&original).is_err());
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

#[test]
fn rendered_runtime_identity_covers_nonzero_generation_image_and_contract() {
    let deployment = HostedDeployment::default();
    let spec = SandboxSpec {
        sandbox_id: "sandbox-1".into(),
        ..SandboxSpec::default()
    };
    let first = deployment.render_runtime(&spec, 1).unwrap();
    let replacement = deployment.render_runtime(&spec, 2).unwrap();

    assert_eq!(first.metadata[RUNTIME_GENERATION_METADATA], "1");
    assert_eq!(replacement.metadata[RUNTIME_GENERATION_METADATA], "2");
    assert_eq!(replacement.env["CHRONO_RUNTIME_GENERATION"], "2");
    assert_eq!(
        replacement.metadata[RUNTIME_IMAGE_METADATA],
        image_sha256(&replacement.image.uri)
    );
    assert_eq!(replacement.metadata[RUNTIME_IMAGE_METADATA].len(), 43);
    assert_eq!(
        replacement.metadata[RUNTIME_CONTRACT_METADATA],
        RUNTIME_CONTRACT_VERSION
    );
    assert_ne!(
        first.metadata[SPEC_FINGERPRINT_METADATA],
        replacement.metadata[SPEC_FINGERPRINT_METADATA]
    );
    assert!(matches!(
        deployment.render_runtime(&spec, 0),
        Err(AdapterError::Rejected(_))
    ));
}
