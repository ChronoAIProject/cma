use cma_sandbox_manager::{
    model::{Change, CreateSandbox, Phase, Resources, RuntimeBinding, Sandbox},
    permissions, schema,
};
use mongodb::bson::{doc, Bson, Document};

fn request() -> CreateSandbox {
    CreateSandbox {
        name: "python-job".into(),
        image: "python:3.13-slim".into(),
        entrypoint: vec!["python".into(), "-c".into(), "print('hello')".into()],
        resources: Resources {
            cpu_millis: 500,
            memory_mib: 512,
        },
        env: [
            ("HOME".into(), "/work".into()),
            ("PATH".into(), "/usr/local/bin:/usr/bin".into()),
        ]
        .into(),
        client_ref: None,
    }
}

fn accepted() -> Sandbox {
    Sandbox::admit("owner", "request", request(), 1).unwrap()
}

#[test]
fn arbitrary_images_and_commands_do_not_require_an_agent_runtime() {
    let mut request = request();
    request.validate().unwrap();
    request.image = "registry.example.com:5000/team/app:release".into();
    request.entrypoint.clear();
    request.validate().unwrap();
    request.image = format!("alpine@sha256:{}", "a".repeat(64));
    request.validate().unwrap();
    request.image = "alpine@sha256:bad".into();
    assert!(request.validate().is_err());
    request.image = "https://registry.example.com/app".into();
    assert!(request.validate().is_err());
}

#[test]
fn request_cannot_supply_owner_or_agent_configuration() {
    for (field, value) in [
        ("owner", "another-user"),
        ("model", "provider-model"),
        ("provider_uid", "replacement"),
    ] {
        let mut body = serde_json::to_value(request()).unwrap();
        body[field] = value.into();
        assert!(
            serde_json::from_value::<CreateSandbox>(body).is_err(),
            "accepted {field}"
        );
    }
}

#[test]
fn interrupted_dispatch_requires_observation_and_cannot_allocate_again() {
    let current = accepted()
        .apply(
            Change::Dispatch {
                id: "dispatch-1".into(),
            },
            2,
        )
        .unwrap();
    // Restart rehydrates exactly the durable phase; a missing provider response
    // cannot mark creation failed/terminated and authorize another allocation.
    let recovered: Sandbox =
        serde_json::from_slice(&serde_json::to_vec(&current).unwrap()).unwrap();
    assert_eq!(recovered.phase, Phase::Dispatching);
    assert!(recovered
        .apply(
            Change::Dispatch {
                id: "dispatch-2".into()
            },
            3
        )
        .is_err());
    assert!(recovered
        .apply(
            Change::Failed {
                detail: "timeout".into()
            },
            3
        )
        .is_err());
    assert!(recovered.apply(Change::Terminate, 3).is_err());
}

#[test]
fn runtime_binding_and_teardown_require_the_original_generation_and_uid() {
    let current = accepted()
        .apply(
            Change::Dispatch {
                id: "dispatch".into(),
            },
            2,
        )
        .unwrap();
    let binding = RuntimeBinding {
        provider_id: "pod".into(),
        provider_uid: "original-uid".into(),
        generation: 1,
        image_digest: format!("sha256:{}", "a".repeat(64)),
    };
    let mut replacement = binding.clone();
    replacement.generation = 2;
    assert!(current
        .apply(
            Change::Bind {
                dispatch_id: "dispatch".into(),
                runtime: replacement
            },
            3
        )
        .is_err());
    assert!(current
        .apply(
            Change::Bind {
                dispatch_id: "other-dispatch".into(),
                runtime: binding.clone()
            },
            3
        )
        .is_err());
    let bound = current
        .apply(
            Change::Bind {
                dispatch_id: "dispatch".into(),
                runtime: binding,
            },
            3,
        )
        .unwrap();
    assert!(bound
        .apply(
            Change::Ready {
                provider_uid: "replacement-uid".into()
            },
            4
        )
        .is_err());
    let ready = bound
        .apply(
            Change::Ready {
                provider_uid: "original-uid".into(),
            },
            4,
        )
        .unwrap();
    let terminating = ready.apply(Change::Terminate, 5).unwrap();
    assert!(terminating
        .apply(
            Change::Terminated {
                provider_uid: "replacement-uid".into()
            },
            6
        )
        .is_err());
    assert_eq!(
        terminating
            .apply(
                Change::Terminated {
                    provider_uid: "original-uid".into()
                },
                6
            )
            .unwrap()
            .phase,
        Phase::Terminated
    );
}

fn authorization(privileges: Vec<Bson>) -> Document {
    doc! { "authInfo": { "authenticatedUsers": [{ "user": "runtime", "db": "test" }], "authenticatedUserPrivileges": privileges } }
}

#[test]
fn runtime_identity_must_have_only_its_explicit_collection_grants() {
    let privileges = schema::runtime_role("test")
        .get_array("privileges")
        .unwrap()
        .clone();
    schema::validate_runtime_privileges(&authorization(privileges.clone()), "test").unwrap();
    for resource in [
        doc! { "db": "test", "collection": "am_agents" },
        doc! { "db": "test", "collection": "eg_channels" },
        doc! { "db": "test", "collection": "sandboxes" },
        doc! { "db": "test", "collection": "" },
        doc! { "db": "", "collection": "" },
        doc! { "anyResource": true },
        doc! { "cluster": true },
    ] {
        let mut expanded = privileges.clone();
        expanded.push(doc! { "resource": resource, "actions": ["find"] }.into());
        assert!(schema::validate_runtime_privileges(&authorization(expanded), "test").is_err());
    }
    let mut migration_grant = privileges.clone();
    migration_grant.push(
        doc! { "resource": { "db": "test", "collection": "sm_sandboxes" }, "actions": ["collMod"] }
            .into(),
    );
    assert!(schema::validate_runtime_privileges(&authorization(migration_grant), "test").is_err());
    assert!(
        schema::validate_runtime_privileges(&authorization(privileges), "different_database")
            .is_err()
    );
    assert!(schema::validate_runtime_privileges(&authorization(vec![]), "test").is_err());
}

#[test]
fn agent_and_legacy_roles_do_not_grant_sandbox_permissions() {
    for foreign in ["cma:*", "cma-am:admin", "cma-eg:admin", "admin", "*"] {
        assert!(!permissions::allows(&[foreign.into()], permissions::CREATE));
    }
    assert!(permissions::allows(
        &[permissions::CREATE.into()],
        permissions::CREATE
    ));
    assert!(!permissions::allows(
        &[permissions::READ.into()],
        permissions::DELETE
    ));
    assert!(!permissions::allows(
        &[permissions::ADMIN.into()],
        "cma-am:messages:write"
    ));
}
