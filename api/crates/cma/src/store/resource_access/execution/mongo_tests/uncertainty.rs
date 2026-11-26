use super::*;
use fixture::{context, generation, Fixture};
use layer2_core::resource_access::{ResourceCapability, ResourceShareStore, ShareChange};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ambiguous_commits_preserve_frozen_members_and_replay_the_exact_final_receipt() {
    let fixture = Fixture::new().await;
    let authority = fixture.authority("recipient").await;
    let member = wire::MemberKey::Execution {
        execution: fixture.execution("lost-reply"),
    };
    fixture.ambiguous_commits(true).await;
    let result = fixture.store.register(&authority, &member).await;
    fixture.ambiguous_commits(false).await;
    assert!(matches!(result, Err(ExecutionError::Unknown)));
    crate::store::test_visibility::majority_barrier(&fixture.client, &fixture.name).await;
    let operation = fixture.freeze().await;
    assert_eq!(operation.frozen_members, 1);
    let key = registration_id(&member).unwrap();
    let document = fixture
        .store
        .collections
        .members
        .find_one(doc! {"_id":key.as_str()})
        .await
        .unwrap()
        .unwrap();
    fixture
        .store
        .collections
        .members
        .delete_one(doc! {"_id":key.as_str()})
        .await
        .unwrap();
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    fixture
        .store
        .collections
        .members
        .insert_one(document)
        .await
        .unwrap();
    let record = fixture.store.record(&key).await.unwrap().unwrap();
    fixture.complete(&operation, &record).await;
    let reopened = MongoResourceExecutionStore {
        sandbox: fixture.sandbox.clone(),
        collections: Collections::new(&fixture.sandbox),
    };
    let resumed = reopened
        .revocation(&operation.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.deadline_unix_ms, operation.deadline_unix_ms);
    fixture.ambiguous_commits(true).await;
    let result = reopened
        .finish_revocation(&operation.key.operation_id)
        .await;
    fixture.ambiguous_commits(false).await;
    assert!(matches!(result, Err(ExecutionError::Unknown)));
    let retained = reopened
        .revocation(&operation.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.key, operation.key);
    assert_eq!(retained.deadline_unix_ms, operation.deadline_unix_ms);
    let replay = reopened
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    assert!(matches!(replay.phase, RevocationPhaseV1::Effective { .. }));
    if matches!(retained.phase, RevocationPhaseV1::Effective { .. }) {
        assert!(retained == replay);
    }
    let observed = reopened
        .revocation(&operation.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert!(observed == replay);
    let repeated = reopened
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    assert!(repeated == replay);
    let original = fixture
        .coordinator
        .begin_revocation(&context("owner"), None, fixture.command("freeze"))
        .await
        .unwrap();
    assert!(replay == original);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mode_change_waits_for_old_generation_and_cannot_rebind_old_execution() {
    let fixture = Fixture::new().await;
    let member = fixture.register("recipient", "old-execution").await;
    let mut change = fixture.command("change-mode");
    change.change = ShareChange::SetMode {
        share_id: fixture.share.id.clone(),
        mode: layer2_core::resource_access::ShareMode::View,
    };
    let operation = fixture
        .coordinator
        .begin_revocation(&context("owner"), None, change)
        .await
        .unwrap();
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    fixture.complete(&operation, &member).await;
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    assert!(fixture
        .coordinator
        .authorize_execution(
            &context("recipient"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::Interact,
            Some(&member.basis)
        )
        .await
        .is_err());
    let read = fixture
        .coordinator
        .authorize_execution(
            &context("recipient"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::ViewHistory,
            None,
        )
        .await
        .unwrap();
    assert_eq!(read.basis().family().unwrap().generation, generation(2));
    assert_ne!(read.basis().family(), member.basis.family());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_activated_latch_fails_closed_instead_of_selecting_legacy() {
    let fixture = Fixture::new().await;
    let operation = fixture.freeze().await;
    fixture
        .store
        .collections
        .grants
        .delete_one(doc! {"_id":"activation"})
        .await
        .unwrap();
    assert!(matches!(
        fixture.store.activation().await,
        Err(ExecutionError::Corrupt)
    ));
    assert!(fixture.sandbox.open_resource_execution().await.is_err());
    assert!(matches!(
        fixture
            .sandbox
            .mutate_share(
                operation
                    .intent
                    .mutation(operation.intent.original_policy_revision)
            )
            .await,
        Err(layer2_core::resource_access::ResourceAccessStoreError::Corrupt)
    ));
    assert!(fixture
        .coordinator
        .authorize_execution(
            &context("recipient"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::Interact,
            None
        )
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_owner_reads_do_not_create_execution_sidecars() {
    use layer2_core::resource_access::{AccessPurpose, ResourceAccessService};
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_execution_legacy_{}", uuid::Uuid::new_v4().simple());
    let (sandbox, target) =
        crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
            &client, &name,
        )
        .await;
    let access = ResourceAccessService::new(Arc::new(sandbox), Arc::new(fixture::Organizations));
    let held = access
        .authorize(
            &context("owner"),
            None,
            &target,
            ResourceCapability::ViewHistory,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    access
        .validate_read_result(&held, &target, ResourceCapability::ViewHistory)
        .await
        .unwrap();
    let names = client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap();
    for name in [
        collections::GRANTS,
        collections::MEMBERS,
        collections::OPERATIONS,
        collections::EFFECTS,
    ] {
        assert!(!names.iter().any(|found| found == name));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_frozen_control_cannot_reopen_an_empty_grant() {
    let fixture = Fixture::new().await;
    let held = fixture.authority("recipient").await;
    let operation = fixture.freeze().await;
    assert_eq!(operation.frozen_members, 0);
    fixture
        .store
        .collections
        .grants
        .delete_one(doc! {
            "_id": operation.key.family.origin.as_str()
        })
        .await
        .unwrap();
    assert!(matches!(
        fixture
            .coordinator
            .authorize_execution(
                &context("recipient"),
                Some("synthetic"),
                &fixture.target,
                ResourceCapability::Interact,
                None,
            )
            .await,
        Err(ExecutionError::Corrupt)
    ));
    assert!(matches!(
        fixture
            .coordinator
            .register_execution(&held, &fixture.execution("late"))
            .await,
        Err(ExecutionError::Corrupt)
    ));
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_operator_authority_survives_share_freeze_without_becoming_owner() {
    use layer2_core::{Authorization, Capability};
    let fixture = Fixture::new().await;
    let mut operator = context("operator");
    operator.caller.authorization = Authorization::new([Capability::ReadAllSandboxes]);
    let before = fixture
        .coordinator
        .authorize_execution(
            &operator,
            None,
            &fixture.target,
            ResourceCapability::ViewHistory,
            None,
        )
        .await
        .expect("independent operator reads remain authorized");
    assert!(before.basis().family().is_none());
    fixture.freeze().await;
    let after = fixture
        .coordinator
        .authorize_execution(
            &operator,
            None,
            &fixture.target,
            ResourceCapability::ViewHistory,
            Some(before.basis()),
        )
        .await
        .expect("share freeze does not revoke independent operator authority");
    assert_eq!(before.basis(), after.basis());
    assert!(fixture
        .coordinator
        .authorize_execution(
            &operator,
            None,
            &fixture.target,
            ResourceCapability::Interact,
            None,
        )
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn request_bound_read_authority_keeps_its_original_expiry_and_read_only_scope() {
    use layer2_core::resource_access::CredentialSource;
    let fixture = Fixture::new().await;
    let mut caller = context("owner");
    caller.credential_source = CredentialSource::RequestBoundRead {
        expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(60),
    };
    let authority = fixture
        .coordinator
        .authorize_execution(
            &caller,
            None,
            &fixture.target,
            ResourceCapability::ViewHistory,
            None,
        )
        .await
        .unwrap();
    assert!(fixture
        .coordinator
        .authorize_execution(
            &caller,
            None,
            &fixture.target,
            ResourceCapability::Interact,
            None,
        )
        .await
        .is_err());
    caller.credential_source = CredentialSource::RequestBoundRead {
        expires_at: std::time::SystemTime::now() - std::time::Duration::from_secs(1),
    };
    assert!(fixture
        .coordinator
        .authorize_execution(
            &caller,
            None,
            &fixture.target,
            ResourceCapability::ViewHistory,
            Some(authority.basis()),
        )
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owning_organization_uses_current_membership_without_becoming_personal_owner() {
    use layer2_core::resource_access::{
        OrganizationOwnershipEvidence, ResourceAccessStore, ResourceOwner,
    };
    let fixture = Fixture::new().await;
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    let mut policy = snapshot.policy.unwrap();
    let organization = snapshot.lineage.sandbox.owner.clone();
    policy.owner = ResourceOwner::Organization {
        subject: organization.clone(),
    };
    policy.organization_evidence = Some(OrganizationOwnershipEvidence {
        issuer: "synthetic".into(),
        organization,
        admitted_by: context("org-admin").actor,
        verified_at: "2026-01-01T00:00:00Z".into(),
    });
    policy.revision += 1;
    fixture
        .sandbox
        .resource_shares
        .delete_many(doc! {})
        .await
        .unwrap();
    fixture
        .sandbox
        .resource_policies
        .replace_one(
            doc! {"_id":policy.sandbox_id.as_str()},
            crate::store::resource_access::records::policy_document(&policy).unwrap(),
        )
        .await
        .unwrap();
    let authority = fixture
        .coordinator
        .authorize_execution(
            &context("org-admin"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::Interact,
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        authority.basis(),
        wire::ExecutionBasisV1::Independent {
            authority: wire::IndependentAuthorityV1::OwningOrganization { .. },
            ..
        }
    ));
    assert!(fixture
        .coordinator
        .authorize_execution(
            &context("org-admin"),
            None,
            &fixture.target,
            ResourceCapability::Interact,
            Some(authority.basis()),
        )
        .await
        .is_err());
    let policy = ResourceExecutionPolicy::enforce(fixture.store.clone(), false)
        .await
        .unwrap();
    let draining = ResourceExecutionCoordinator::new(fixture.access.clone(), policy);
    draining
        .authorize_execution(
            &context("org-admin"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::Interact,
            Some(authority.basis()),
        )
        .await
        .unwrap();
    fixture.cleanup().await;
}
