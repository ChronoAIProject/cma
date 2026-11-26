use layer2_core::ids::{SandboxId, SandboxProfileId, ServiceId, SubjectId};
use layer2_core::profiles::{
    content_digest, AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
};
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxEventStore, SandboxStore, SandboxStoreError,
    SandboxTransaction, SandboxTransactionStore, WorkAuthorityProtocol,
};
use layer2_core::sandbox::{
    Sandbox, SandboxCredentialState, SandboxEventKind, SandboxLimits, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use mongodb::bson::{doc, Bson};

use super::sandboxes::{
    admitted_sandbox_document, read_sandbox, read_sandbox_admission_replay, sandbox_document,
    sandbox_indexes, sandbox_replacement_fields,
};

const NOW: &str = "2026-09-05T00:00:00Z";

fn sandbox(origin: SandboxOrigin) -> Sandbox {
    let content = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1_000,
        memory_mib: 2_048,
        nyxid_services: vec![ServiceId::new("profile-service")],
        agent_provider: AgentProvider::Codex,
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 4,
        max_agents_per_workspace: 4,
        workspace_profiles: vec![],
        software_install_commands: vec![],
        environment_variables: vec![],
    };
    Sandbox {
        id: SandboxId("sbx_schema".into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_schema".into()),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            always_on: None,
            content,
        },
        status: SandboxStatus::Provisioning,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![ServiceId::new("profile-service")],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn trigger_document(sandbox: &Sandbox) -> (mongodb::bson::Document, SandboxAdmission) {
    let admission = SandboxAdmission::new(
        sandbox.clone(),
        SandboxAdmissionName::Allocate("research".into()),
        1,
        NOW.into(),
    )
    .unwrap();
    let document = admitted_sandbox_document(
        sandbox,
        1,
        &admission.name,
        &admission.admission_fingerprint,
    )
    .unwrap();
    (document, admission)
}

#[test]
fn manual_origin_query_fields_are_explicit_and_fingerprint_free() {
    let sandbox = sandbox(SandboxOrigin::Manual);
    let document = sandbox_document(&sandbox, 0).unwrap();

    assert_eq!(document.get_str("origin_type").unwrap(), "manual");
    assert_eq!(document.get("origin_launch_id"), Some(&Bson::Null));
    assert_eq!(document.get("admission_name"), Some(&Bson::Null));
    assert_eq!(document.get("admission_fingerprint"), Some(&Bson::Null));
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
    assert!(read_sandbox_admission_replay(&document).is_err());
}

#[test]
fn prepared_proof_envelope_round_trips_with_authority_off_or_on() {
    for (authority_protocol, proof, expected_version) in [
        (None, None, 1_i64),
        (Some(WorkAuthorityProtocol::RenewedV1), None, 3_i64),
        (None, Some("a".repeat(64)), 4_i64),
        (
            Some(WorkAuthorityProtocol::RenewedV1),
            Some("b".repeat(64)),
            4_i64,
        ),
    ] {
        let mut value = sandbox(SandboxOrigin::Manual);
        value.work.authority_protocol = authority_protocol;
        value.work.prepared_installation_digest = proof;
        let document = sandbox_document(&value, 0).unwrap();
        assert_eq!(
            document.get_i64("document_schema_version").unwrap(),
            expected_version
        );
        assert_eq!(read_sandbox(&document).unwrap(), value);
    }
}

#[test]
fn trigger_origin_and_fingerprint_round_trip_and_survive_replacement() {
    let sandbox = sandbox(SandboxOrigin::Trigger {
        launch_id: "launch-schema".into(),
    });
    let (document, admission) = trigger_document(&sandbox);

    assert_eq!(document.get_str("origin_type").unwrap(), "trigger");
    assert_eq!(
        document.get_str("origin_launch_id").unwrap(),
        "launch-schema"
    );
    assert_eq!(
        document.get_str("admission_fingerprint").unwrap(),
        admission.admission_fingerprint
    );
    assert_eq!(
        document.get_document("admission_name").unwrap(),
        &doc! { "mode": "allocate", "value": "research" }
    );
    let record = document.get_document("record").unwrap();
    assert_eq!(record.get_str("owner").unwrap(), "owner");
    assert_eq!(
        record.get_document("origin").unwrap(),
        &doc! { "type": "trigger", "launch_id": "launch-schema" }
    );
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
    let replay = read_sandbox_admission_replay(&document).unwrap();
    assert_eq!(replay.sandbox, sandbox);
    assert_eq!(replay.name, admission.name);
    assert_eq!(
        replay.admission_fingerprint,
        admission.admission_fingerprint
    );
    let replacement = sandbox_replacement_fields(&sandbox).unwrap();
    assert!(!replacement.contains_key("admission_name"));
    assert!(!replacement.contains_key("admission_fingerprint"));
    assert_eq!(replacement.get_str("origin_type").unwrap(), "trigger");
    assert!(sandbox_document(&sandbox, 0).is_err());
}

#[test]
fn trigger_query_fields_and_fingerprint_are_independently_validated() {
    let sandbox = sandbox(SandboxOrigin::Trigger {
        launch_id: "launch-schema".into(),
    });
    let (document, _) = trigger_document(&sandbox);
    for (field, value) in [
        ("origin_type", Bson::String("manual".into())),
        ("origin_launch_id", Bson::String("another-launch".into())),
        ("admission_name", Bson::Null),
        (
            "admission_name",
            Bson::Document(doc! { "mode": "allocate", "value": "changed" }),
        ),
        (
            "admission_name",
            Bson::Document(doc! { "mode": "allocate", "value": "research", "unexpected": true }),
        ),
        ("admission_fingerprint", Bson::Null),
        ("admission_fingerprint", Bson::String("C".repeat(64))),
    ] {
        let mut corrupt = document.clone();
        corrupt.insert(field, value);
        assert!(read_sandbox(&corrupt).is_err(), "accepted corrupt {field}");
    }
}

#[test]
fn sandbox_indexes_match_the_schema_contract() {
    let indexes = sandbox_indexes();
    assert_eq!(indexes.len(), 12);
    assert_eq!(indexes[11].keys, doc! { "retired_at": 1, "_id": 1 });
    assert_eq!(indexes[0].keys, doc! { "owner": 1, "name": 1 });
    let options = indexes[0].options.as_ref().unwrap();
    assert_eq!(options.unique, Some(true));
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "retired_at": Bson::Null })
    );
    assert_eq!(indexes[1].keys, doc! { "owner": 1, "origin_launch_id": 1 });
    let options = indexes[1].options.as_ref().unwrap();
    assert_eq!(options.unique, Some(true));
    assert_eq!(
        options.name.as_deref(),
        Some("sandboxes_owner_trigger_launch")
    );
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "origin_type": "trigger" })
    );
    assert_eq!(
        indexes[2].keys,
        doc! { "record.owner": 1, "record.origin.launch_id": 1 }
    );
    let options = indexes[2].options.as_ref().unwrap();
    assert_eq!(options.unique, Some(true));
    assert_eq!(
        options.name.as_deref(),
        Some("sandboxes_record_owner_trigger_launch")
    );
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "record.origin.type": "trigger" })
    );
    assert_eq!(
        indexes[3].keys,
        doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 }
    );
    assert_eq!(
        indexes[4].keys,
        doc! { "owner": 1, "status": 1, "updated_at": -1 }
    );
    assert_eq!(
        indexes[5].keys,
        doc! {
            "owner": 1,
            "status": 1,
            "runtime_version": 1,
            "_id": 1,
        }
    );
    assert_eq!(
        indexes[5].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_operator_inventory")
    );
    assert_eq!(indexes[6].keys, doc! { "owner": 1, "_id": 1 });
    assert_eq!(
        indexes[6].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_owner_page")
    );
    assert_eq!(indexes[7].keys, doc! { "owner": 1, "status": 1, "_id": 1 });
    assert_eq!(
        indexes[7].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_owner_status_page")
    );
    assert_eq!(indexes[8].keys, doc! { "status": 1, "_id": 1 });
    assert_eq!(
        indexes[8].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_maintenance_page")
    );
}

async fn save_generation_successor(
    store: &super::sandbox_mongo::MongoSandboxStore,
    expected_revision: u64,
    sandbox: Sandbox,
    transactional: bool,
) -> Result<(), SandboxStoreError> {
    if transactional {
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision,
                sandbox: Box::new(sandbox),
                event_at: NOW.into(),
                event: SandboxEventKind::Rotated {
                    runtime_version: "image-v2".into(),
                },
            })
            .await
    } else {
        store.save_sandbox(expected_revision, sandbox).await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_generic_saves_preserve_runtime_generation_authority() {
    use super::sandbox_maintenance::mongo_integration_tests::{database, fixture};

    let (client, name, store) = database().await;
    for transactional in [false, true] {
        let (sandbox, _) = fixture(&store, &format!("sbx_generation_{transactional}")).await;
        let binding = sandbox.runtime.as_ref().unwrap();
        for (runtime_id, generation, high_water) in [
            (None, None, 0),
            (Some(binding.runtime_id.as_str()), None, 1),
            (Some(binding.runtime_id.as_str()), Some(2), 2),
            (Some("unknown-replacement"), None, 1),
        ] {
            let mut next = sandbox.clone();
            next.revision += 1;
            next.lifecycle.runtime_generation_high_water = high_water;
            next.runtime = runtime_id.map(|id| {
                let mut runtime = binding.clone();
                runtime.runtime_id = id.into();
                runtime.provision_generation = generation;
                runtime
            });
            assert_eq!(
                save_generation_successor(&store, sandbox.revision, next, transactional).await,
                Err(SandboxStoreError::Conflict),
                "accepted runtime {runtime_id:?}, generation {generation:?}, high-water {high_water}"
            );
            assert_eq!(
                store.get_sandbox(&sandbox.id).await.unwrap(),
                Some(sandbox.clone())
            );
            assert!(store
                .sandbox_events(&sandbox.id, 0, 10)
                .await
                .unwrap()
                .is_empty());
        }

        let mut replacement = sandbox.clone();
        replacement.revision += 1;
        replacement.lifecycle.runtime_generation_high_water = 2;
        let runtime = replacement.runtime.as_mut().unwrap();
        runtime.runtime_id = "replacement-generation-2".into();
        runtime.runtime_version = "image-v2".into();
        runtime.provision_generation = Some(2);
        save_generation_successor(&store, sandbox.revision, replacement.clone(), transactional)
            .await
            .unwrap();
        assert_eq!(
            store.get_sandbox(&sandbox.id).await.unwrap(),
            Some(replacement.clone())
        );
        let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
        assert_eq!(events.len(), usize::from(transactional));
        if transactional {
            assert_eq!(events[0].seq, 1);
            assert_eq!(
                events[0].kind,
                SandboxEventKind::Rotated {
                    runtime_version: "image-v2".into()
                }
            );
        }

        let mut stale = replacement.clone();
        stale.revision += 1;
        stale.runtime = sandbox.runtime;
        assert_eq!(
            save_generation_successor(&store, replacement.revision, stale, transactional).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.get_sandbox(&replacement.id).await.unwrap(),
            Some(replacement.clone())
        );
        assert_eq!(
            store.sandbox_events(&replacement.id, 0, 10).await.unwrap(),
            events
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[test]
fn always_on_policy_changes_preserve_original_admission_replay() {
    for origin in [
        SandboxOrigin::Lifecycle {
            operation_id: "operation-always-on".into(),
        },
        SandboxOrigin::Trigger {
            launch_id: "launch-always-on".into(),
        },
    ] {
        for original_policy in [None, Some(false), Some(true)] {
            let mut original = sandbox(origin.clone());
            original.profile.always_on = original_policy;
            let (mut document, admission) = trigger_document(&original);
            for policy in [Some(true), Some(false), None] {
                let mut next = original.clone();
                next.profile.always_on = policy;
                next.revision += 1;
                document.extend(sandbox_replacement_fields(&next).unwrap());
                let replay = read_sandbox_admission_replay(&document).unwrap();
                assert_eq!(replay.sandbox, next);
                assert_eq!(replay.name, admission.name);
                assert_eq!(
                    replay.admission_fingerprint,
                    admission.admission_fingerprint
                );
                replay.validate().unwrap();
                assert_eq!(read_sandbox(&document).unwrap(), next);
            }
        }
    }
}
