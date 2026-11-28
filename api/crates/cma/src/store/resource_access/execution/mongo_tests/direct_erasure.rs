use super::{fixture::*, *};
use crate::store::account_erasure::test_drain;
use layer2_core::{account_deletion::ErasureInventory, resource_access::*, sandbox::SandboxStatus};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn indexed_execution_erasure_preserves_foreign_grants_and_is_idempotent() {
    let fixture = Fixture::with_erasure_keys(true).await;
    let db = fixture.client.database(&fixture.name);
    let mut caller = context("delegated-subject");
    caller.actor = ResourceActor::Api {
        subject: caller.caller.subject.clone(),
        delegation_id: Some("synthetic-delegation".into()),
    };
    caller.credential_source = CredentialSource::delegated("synthetic-delegation");
    let authority = fixture
        .coordinator
        .authorize_execution(
            &caller,
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::Interact,
            None,
        )
        .await
        .unwrap();
    let ticket = fixture
        .started_ticket_for(&authority, "delegated-turn")
        .await;
    let sibling = fixture
        .register("another-recipient", "unrelated-turn")
        .await;
    let intent = wire::EffectIntentV1 {
        effect_id: id("synthetic-effect"),
        execution: ticket.execution.clone(),
        registration: ticket.registration.clone(),
        grant: ticket.basis.clone(),
        operation: wire::EffectOperationClass::ToolRequest,
        request_digest: digest('d'),
        provider_idempotency: None,
    };
    let receiver = &ticket.execution.receiver;
    let permission = fixture
        .coordinator
        .prepare_effect(receiver, &intent)
        .await
        .unwrap();
    fixture
        .coordinator
        .observe_effect(
            receiver,
            &wire::EffectObservationV1 {
                protocol: 1,
                permission_id: permission.permission_id,
                execution: intent.execution,
                effect_id: intent.effect_id,
                sequence: generation(1),
                outcome: wire::EffectOutcome::CommitConfirmed,
                evidence_digest: digest('e'),
            },
        )
        .await
        .unwrap();
    let members = &fixture.store.collections.members;
    let member = members
        .find_one(doc! {"_id":ticket.registration.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        member
            .get_document(records::ERASURE_KEYS)
            .unwrap()
            .get_str("actor_subject")
            .unwrap(),
        "delegated-subject"
    );
    assert!(records::read_member(&member).unwrap().permit.is_some());
    let sandbox_id = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap()
        .lineage
        .sandbox
        .id;
    let subjects_sandboxes = Default::default();
    let scope = erasure::Scope {
        subject: "delegated-subject",
        sandboxes: &subjects_sandboxes,
        organization_sandboxes: &subjects_sandboxes,
    };
    assert!(matches!(
        erasure::erase(&db, erasure::MEMBERS, ticket.registration.as_str(), &scope).await,
        Err(erasure::EraseError::Retained)
    ));
    let reopened = fixture
        .sandbox
        .open_resource_execution(false)
        .await
        .unwrap();
    let policy = ResourceExecutionPolicy::enforce(reopened, true)
        .await
        .unwrap();
    let coordinator = ResourceExecutionCoordinator::new(fixture.access.clone(), policy);
    let operation = coordinator
        .begin_revocation(
            &context("owner"),
            None,
            fixture.command("erase-after-revoke"),
        )
        .await
        .unwrap();
    let chosen = fixture
        .store
        .record(&ticket.registration)
        .await
        .unwrap()
        .unwrap();
    fixture.complete(&operation, &chosen).await;
    fixture.complete(&operation, &sibling).await;
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();

    let mut actor_inventory = ErasureInventory::default();
    let removed = test_drain(&db, "delegated-subject", &mut actor_inventory).await;
    assert_eq!(removed.get(erasure::EFFECTS), Some(&1));
    assert_eq!(removed.get(erasure::MEMBERS), Some(&1));
    assert_eq!(removed.get(erasure::GRANTS), None);
    assert_eq!(removed.get(erasure::OPERATIONS), None);
    assert!(fixture
        .store
        .record(&ticket.registration)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .effects_for(&ticket.registration, 0, 8)
        .await
        .unwrap()
        .is_empty());
    let remaining = fixture
        .store
        .frozen_members(&operation.key.operation_id, 0, 8)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].registration, sibling.registration);
    let repeat = test_drain(&db, "delegated-subject", &mut actor_inventory).await;
    assert!(erasure::COLLECTIONS
        .iter()
        .all(|collection| !repeat.contains_key(*collection)));

    let raw = fixture
        .sandbox
        .sandboxes
        .find_one(doc! {"_id":sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut sandbox = crate::store::sandboxes::read_sandbox(&raw).unwrap();
    sandbox.status = SandboxStatus::Retired;
    sandbox.retired_at = Some("2026-01-01T00:00:00Z".into());
    sandbox.purge_after = sandbox.retired_at.clone();
    sandbox.runtime = None;
    sandbox.credential.current = None;
    sandbox.credential.delivered = false;
    sandbox.work = layer2_core::sandbox::SandboxWorkState::new(
        layer2_core::sandbox::SandboxWorkStep::None,
        sandbox.retired_at.clone().unwrap(),
    );
    fixture
        .sandbox
        .sandboxes
        .replace_one(
            doc! {"_id":sandbox_id.as_str()},
            crate::store::sandboxes::sandbox_document(&sandbox, 0).unwrap(),
        )
        .await
        .unwrap();
    let mut owner_inventory = ErasureInventory::default();
    let removed = test_drain(&db, "owner", &mut owner_inventory).await;
    assert_eq!(removed.get(erasure::MEMBERS), Some(&1));
    assert_eq!(removed.get(erasure::GRANTS), Some(&2));
    assert_eq!(removed.get(erasure::OPERATIONS), Some(&1));
    let grants = &fixture.store.collections.grants;
    assert_eq!(grants.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(members.count_documents(doc! {}).await.unwrap(), 0);
    let repeat = test_drain(&db, "owner", &mut owner_inventory).await;
    assert!(erasure::COLLECTIONS
        .iter()
        .all(|collection| !repeat.contains_key(*collection)));
    fixture.cleanup().await;
}
