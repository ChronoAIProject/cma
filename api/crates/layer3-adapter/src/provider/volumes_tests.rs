use super::*;

pub(super) fn volume() -> ProviderVolume {
    ProviderVolume {
        name: "home".into(),
        pvc: ProviderPersistentVolumeClaim {
            claim_name: "cma-home-test".into(),
            create_if_not_exists: false,
            delete_on_sandbox_termination: false,
        },
        mount_path: "/home/cma".into(),
        read_only: false,
        sub_path: Some("home".into()),
    }
}

#[test]
fn mounted_home_uses_the_existing_claim_wire_contract() {
    let encoded = serde_json::to_value(volume()).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "name":"home", "pvc":{"claimName":"cma-home-test","createIfNotExists":false,"deleteOnSandboxTermination":false},
            "mountPath":"/home/cma", "readOnly":false, "subPath":"home"
        })
    );
    assert!(serde_json::from_value::<ProviderVolume>(serde_json::json!({
        "name":"home", "host":{"path":"/"}, "mountPath":"/home/cma", "readOnly":false
    }))
    .is_err());
}

#[test]
fn volume_admission_refuses_implicit_allocation_escaping_and_overlapping_paths() {
    for invalid in ["", "UPPER", "-home", "home-", "home/other", "home.other"] {
        let mut changed = volume();
        changed.pvc.claim_name = invalid.into();
        assert!(validate(&[changed]).is_err(), "{invalid}");
    }
    for invalid in [
        "/",
        "relative",
        "/home/../etc",
        "/home//cma",
        "/home/cma/",
        "/home/./cma",
    ] {
        let mut changed = volume();
        changed.mount_path = invalid.into();
        assert!(validate(&[changed]).is_err(), "{invalid}");
    }
    for invalid in [
        "",
        "/home",
        "../home",
        "home/../state",
        "home\\state",
        "home\nstate",
    ] {
        let mut changed = volume();
        changed.sub_path = Some(invalid.into());
        assert!(validate(&[changed]).is_err(), "{invalid}");
    }
    let mut automatic = volume();
    automatic.pvc.create_if_not_exists = true;
    assert!(validate(&[automatic]).is_err());
    let mut automatic_delete = volume();
    automatic_delete.pvc.delete_on_sandbox_termination = true;
    assert!(validate(&[automatic_delete]).is_err());
    for mount in ["/home", "/home/cma", "/home/cma/child"] {
        let mut nested = volume();
        nested.name = "second".into();
        nested.mount_path = mount.into();
        assert!(validate(&[volume(), nested]).is_err());
    }
    let mut duplicate = volume();
    duplicate.mount_path = "/var/lib/cma-runtime".into();
    assert!(validate(&[volume(), duplicate]).is_err());
}

#[test]
fn one_claim_can_supply_separate_home_and_runtime_state_mounts() {
    let mut state = volume();
    state.name = "runtime-state".into();
    state.mount_path = "/var/lib/cma-runtime".into();
    state.sub_path = Some("state".into());
    assert!(validate(&[volume(), state]).is_ok());
    let mut sibling = volume();
    sibling.name = "sibling".into();
    sibling.mount_path = "/home/cma-backup".into();
    sibling.read_only = true;
    assert!(validate(&[volume(), sibling]).is_ok());
}
