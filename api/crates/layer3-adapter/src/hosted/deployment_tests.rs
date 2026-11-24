use super::*;

#[test]
fn only_explicit_retained_tool_authority_changes_the_runtime_specification() {
    let deployment = HostedDeployment::default();
    let spec = SandboxSpec {
        sandbox_id: "ordinary-sandbox".into(),
        ..SandboxSpec::default()
    };
    let old = deployment.render(&spec).unwrap();
    assert!(!old.env.contains_key("CHRONO_TOOL_PROXY_URL"));
    let old_json = serde_json::to_value(&spec).unwrap();
    assert!(old_json.get("tool_proxy_url").is_none());
    let recovered: SandboxSpec = serde_json::from_value(old_json).unwrap();
    assert_eq!(deployment.render(&recovered).unwrap(), old);
    let mut supplied = spec.clone();
    supplied.environment.insert(
        "CHRONO_TOOL_PROXY_URL".into(),
        "https://untrusted.example/mcp".into(),
    );
    assert!(deployment.render(&supplied).is_err());
    let mut native = spec;
    native.tool_proxy_url = Some("https://gateway.example/mcp".into());
    let retained: SandboxSpec =
        serde_json::from_value(serde_json::to_value(&native).unwrap()).unwrap();
    let rendered = deployment.render(&retained).unwrap();
    assert_eq!(
        rendered.env["CHRONO_TOOL_PROXY_URL"],
        "https://gateway.example/mcp"
    );
    assert_ne!(
        rendered.metadata[SPEC_FINGERPRINT_METADATA],
        old.metadata[SPEC_FINGERPRINT_METADATA]
    );
    let mut expected = old;
    expected.env.insert(
        "CHRONO_TOOL_PROXY_URL".into(),
        "https://gateway.example/mcp".into(),
    );
    expected.metadata.insert(
        SPEC_FINGERPRINT_METADATA.into(),
        rendered.metadata[SPEC_FINGERPRINT_METADATA].clone(),
    );
    assert_eq!(rendered, expected);
}

fn request() -> ProviderSandboxRequest {
    ProviderSandboxRequest {
        funded_until: None,
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
fn managed_llm_route_follows_pinned_service_scope_during_creation_and_revival() {
    for configured_service in ["chrono-llm", "chrono-llm-public"] {
        let deployment = HostedDeployment {
            upstream_base_url: format!("https://nyx.example/api/v1/proxy/s/{configured_service}"),
            platform_base_url: "https://nyx.example".into(),
            ..HostedDeployment::default()
        };
        for (services, expected) in [
            (vec!["chrono-llm", "ornn-api", "api-github"], "chrono-llm"),
            (
                vec!["chrono-llm-public", "ornn-api", "api-github"],
                "chrono-llm-public",
            ),
            (vec!["chrono-llm", "chrono-llm-public"], "chrono-llm-public"),
        ] {
            let spec = SandboxSpec {
                sandbox_id: "sandbox-model-scope".into(),
                reaches: services.into_iter().map(str::to_owned).collect(),
                ..SandboxSpec::default()
            };
            for generation in [1, 2] {
                let rendered = deployment.render_runtime(&spec, generation).unwrap();
                assert_eq!(
                    rendered.env["CHRONO_UPSTREAM_BASE_URL"],
                    format!("https://nyx.example/api/v1/proxy/s/{expected}")
                );
                assert_eq!(
                    rendered.env["CHRONO_SANDBOX_REACHES"],
                    spec.reaches.join(",")
                );
            }
            let own = SandboxSpec {
                own_credential: true,
                ..spec
            };
            assert_eq!(
                deployment.render(&own).unwrap().env["CHRONO_UPSTREAM_BASE_URL"],
                ""
            );
        }
    }
}

#[test]
fn public_llm_selection_preserves_custom_upstreams() {
    let spec = SandboxSpec {
        sandbox_id: "sandbox-model-scope".into(),
        reaches: vec!["chrono-llm-public".into()],
        ..SandboxSpec::default()
    };
    for upstream in [
        "https://models.example/v1",
        "https://other.example/api/v1/proxy/s/chrono-llm",
        "https://nyx.example/api/v1/proxy/s/custom-model",
    ] {
        let deployment = HostedDeployment {
            upstream_base_url: upstream.into(),
            platform_base_url: "https://nyx.example".into(),
            ..HostedDeployment::default()
        };
        assert_eq!(
            deployment.render(&spec).unwrap().env["CHRONO_UPSTREAM_BASE_URL"],
            upstream
        );
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
fn retained_startup_policy_successor_changes_only_the_admitted_fields() {
    use crate::provider::{ProviderPersistentVolumeClaim, ProviderVolume};

    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let mut predecessor = request();
    predecessor.image.uri = image.clone();
    predecessor.volumes.push(ProviderVolume {
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
    predecessor
        .metadata
        .insert("cma/home-identity".into(), "a".repeat(43));
    predecessor
        .metadata
        .insert(RUNTIME_GENERATION_METADATA.into(), "2".into());
    predecessor
        .metadata
        .insert(RUNTIME_IMAGE_METADATA.into(), image_sha256(&image));
    predecessor
        .env
        .insert("CHRONO_RUNTIME_GENERATION".into(), "2".into());
    predecessor
        .env
        .insert("CHRONO_SANDBOX_REACHES".into(), "chrono-llm-public".into());
    predecessor.env.insert(
        "CHRONO_UPSTREAM_BASE_URL".into(),
        "https://model.example/v1".into(),
    );
    predecessor.metadata.insert(
        SPEC_FINGERPRINT_METADATA.into(),
        provider_request_fingerprint(&predecessor).unwrap(),
    );
    let target = format!("registry.example/runtime@sha256:{}", "b".repeat(64));

    for (image, enabled) in [(&image, true), (&target, false)] {
        let successor = predecessor
            .retained_startup_policy_successor(image, 5, enabled)
            .unwrap();
        let mut expected = predecessor.clone();
        expected.image.uri.clone_from(image);
        expected
            .env
            .insert("CHRONO_RUNTIME_GENERATION".into(), "5".into());
        expected.env.insert(
            "CHRONO_RUNTIME_LEGACY_JOURNAL_RECOVERY_ENABLED".into(),
            enabled.to_string(),
        );
        expected
            .metadata
            .insert(RUNTIME_GENERATION_METADATA.into(), "5".into());
        expected
            .metadata
            .insert(RUNTIME_IMAGE_METADATA.into(), image_sha256(image));
        expected.metadata.remove(SPEC_FINGERPRINT_METADATA);
        expected.metadata.insert(
            SPEC_FINGERPRINT_METADATA.into(),
            provider_request_fingerprint(&expected).unwrap(),
        );
        assert_eq!(successor, expected);
    }

    assert!(predecessor
        .retained_startup_policy_successor(&target, 2, false)
        .is_err());
    predecessor.env.insert(
        "CHRONO_SANDBOX_REACHES".into(),
        "changed-after-freeze".into(),
    );
    assert!(predecessor
        .retained_startup_policy_successor(&target, 5, false)
        .is_err());
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

#[test]
fn new_work_renders_its_admitted_release_and_recorded_images_win() {
    let bootstrap = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let released = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let recorded = format!("registry.example/runtime@sha256:{}", "c".repeat(64));
    let deployment = HostedDeployment {
        image: crate::released_image(bootstrap.clone()),
        ..HostedDeployment::default()
    };
    let spec = SandboxSpec {
        sandbox_id: "new-work".into(),
        ..SandboxSpec::default()
    };
    let release = crate::ReleasedRuntime {
        revision: 7,
        agent: released.clone(),
        machine: None,
        lane_revisions: None,
    };
    // A request without a captured release keeps the bootstrap image.
    let legacy = crate::RuntimeProvisionRequest::new(1, spec.clone());
    assert_eq!(
        deployment.render_request(&legacy).unwrap().image.uri,
        bootstrap
    );
    // New work renders the release it was admitted against.
    let admitted =
        crate::RuntimeProvisionRequest::new(1, spec.clone()).with_released(release.clone());
    assert_eq!(
        deployment.render_request(&admitted).unwrap().image.uri,
        released
    );
    // The snapshot never enters the receipt identity.
    assert_eq!(
        serde_json::to_value(&admitted).unwrap(),
        serde_json::to_value(&legacy).unwrap()
    );
    // A recorded image (an existing home or custom image) always wins.
    let mut pinned = spec;
    pinned.runtime_image = Some(recorded.clone());
    let pinned = crate::RuntimeProvisionRequest::new(1, pinned).with_released(release.clone());
    assert_eq!(
        deployment.render_request(&pinned).unwrap().image.uri,
        recorded
    );
    // Only this release's lanes carry the deployment contract.
    assert_eq!(
        release.image(&released),
        Some(crate::released_image(released))
    );
    assert_eq!(release.image(&recorded), None);
}
