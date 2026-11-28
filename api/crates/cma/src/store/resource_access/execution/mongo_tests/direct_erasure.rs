use super::{fixture::*, *};
use crate::store::account_erasure::{test_try_drain_execution, CohortBlock, Error};
use layer2_core::{
    account_deletion::{ErasureBlock, ErasureInventory},
    resource_access::{
        revocations::{ResourceRevocationService, ResourceRevocations, RevocationStage},
        *,
    },
    sandbox::SandboxStatus,
};

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
    let sibling_ticket = fixture
        .started_ticket_for(
            &fixture.authority("another-recipient").await,
            "unrelated-turn",
        )
        .await;
    let settled = prepared_effect(&fixture, &ticket).await;
    observe(&fixture, &settled).await;
    let sibling_effect = prepared_effect(&fixture, &sibling_ticket).await;
    let unknown = wire::EffectObservationV1 {
        outcome: wire::EffectOutcome::SubmissionUnknown,
        ..sibling_effect.clone()
    };
    observe(&fixture, &unknown).await;
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
    let sibling = fixture
        .store
        .record(&sibling_ticket.registration)
        .await
        .unwrap()
        .unwrap();
    fixture.complete(&operation, &chosen).await;
    fixture.complete(&operation, &sibling).await;
    let effective = fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    let RevocationPhaseV1::Effective { receipt } = effective.phase else {
        panic!("expected effective revocation")
    };
    let service = ResourceRevocationService::new(fixture.access.clone(), fixture.store.clone())
        .await
        .unwrap();
    let owner = context("owner");
    let status = || {
        service.status(
            &owner,
            None,
            &fixture.selector,
            operation.key.operation_id.as_str(),
        )
    };
    let before = status().await.unwrap();
    assert_eq!(
        (
            before.participants,
            before.stopped,
            before.unresolved_effects
        ),
        (2, 2, 1)
    );

    let mut actor_inventory = ErasureInventory::default();
    let error = test_try_drain_execution(&db, "delegated-subject", &mut actor_inventory, false)
        .await
        .unwrap_err();
    assert!(matches!(
        erasure_cause(&error),
        Error::CohortBlocked(CohortBlock::ExecutionErasureQualificationRequired)
    ));
    let current = fixture
        .store
        .revocation(&operation.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert!(current.erased_ordinals.is_empty());
    assert!(fixture
        .store
        .record(&ticket.registration)
        .await
        .unwrap()
        .is_some());
    let removed = test_try_drain_execution(&db, "delegated-subject", &mut actor_inventory, true)
        .await
        .unwrap();
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
    let repeat = test_try_drain_execution(&db, "delegated-subject", &mut actor_inventory, true)
        .await
        .unwrap();
    assert!(erasure::COLLECTIONS
        .iter()
        .all(|collection| !repeat.contains_key(*collection)));

    let after = status().await.unwrap();
    assert_eq!(after.stage, RevocationStage::Effective);
    assert_eq!(
        (after.participants, after.stopped, after.unresolved_effects),
        (2, 2, 1)
    );
    assert_eq!(after.receipt.as_ref(), Some(receipt.as_ref()));
    members.insert_one(member.clone()).await.unwrap();
    assert!(matches!(status().await, Err(ExecutionError::Corrupt)));
    members
        .delete_one(doc! {"_id":ticket.registration.as_str()})
        .await
        .unwrap();
    let retained = members
        .find_one_and_delete(doc! {"_id":sibling.registration.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(status().await, Err(ExecutionError::Corrupt)));
    members.insert_one(retained).await.unwrap();

    retire(&fixture, sandbox_id.as_str()).await;
    let mut owner_inventory = ErasureInventory::default();
    assert_retained(
        test_try_drain_execution(&db, "owner", &mut owner_inventory, true)
            .await
            .unwrap_err(),
    );
    assert_eq!(
        fixture
            .store
            .effects_for(&sibling.registration, 0, 8)
            .await
            .unwrap()[0]
            .observations,
        vec![unknown]
    );
    assert!(fixture
        .store
        .revocation(&operation.key.operation_id)
        .await
        .unwrap()
        .is_some());
    observe(
        &fixture,
        &wire::EffectObservationV1 {
            sequence: generation(2),
            ..sibling_effect
        },
    )
    .await;
    let removed = test_try_drain_execution(&db, "owner", &mut owner_inventory, true)
        .await
        .unwrap();
    assert_eq!(removed.get(erasure::MEMBERS), Some(&1));
    assert_eq!(removed.get(erasure::GRANTS), Some(&2));
    assert_eq!(removed.get(erasure::OPERATIONS), Some(&1));
    assert_eq!(removed.get(erasure::EFFECTS), Some(&1));
    let grants = &fixture.store.collections.grants;
    assert_eq!(grants.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(members.count_documents(doc! {}).await.unwrap(), 0);
    let repeat = test_try_drain_execution(&db, "owner", &mut owner_inventory, true)
        .await
        .unwrap();
    assert!(erasure::COLLECTIONS
        .iter()
        .all(|collection| !repeat.contains_key(*collection)));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn independent_execution_erasure_waits_for_provider_settlement() {
    let fixture = Fixture::with_erasure_keys(true).await;
    let db = fixture.client.database(&fixture.name);
    let authority = fixture.authority("owner").await;
    assert!(authority.basis().family().is_none());
    let ticket = fixture.started_ticket_for(&authority, "owner-turn").await;
    let terminal = prepared_effect(&fixture, &ticket).await;
    let sandbox_id = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap()
        .lineage
        .sandbox
        .id;
    retire(&fixture, sandbox_id.as_str()).await;
    let mut inventory = ErasureInventory::default();
    assert_retained(
        test_try_drain_execution(&db, "owner", &mut inventory, true)
            .await
            .unwrap_err(),
    );
    let pending = fixture
        .store
        .effects_for(&ticket.registration, 0, 8)
        .await
        .unwrap();
    assert!(pending[0].observations.is_empty());
    observe(&fixture, &terminal).await;
    let removed = test_try_drain_execution(&db, "owner", &mut inventory, true)
        .await
        .unwrap();
    assert_eq!(removed.get(erasure::MEMBERS), Some(&1));
    assert_eq!(removed.get(erasure::EFFECTS), Some(&1));
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
    fixture.cleanup().await;
}

fn erasure_cause(error: &Error) -> &Error {
    match error {
        Error::Interrupted { error, .. } => error.as_ref(),
        other => other,
    }
}

pub(super) fn assert_retained(error: Error) {
    let cause = erasure_cause(&error);
    assert!(
        matches!(cause, Error::Blocked(block)
        if matches!(&block.reason, ErasureBlock::UnsupportedResidue { .. })),
        "{error:?}"
    );
}

async fn prepared_effect(
    fixture: &Fixture,
    ticket: &wire::ExecutionTicketV1,
) -> wire::EffectObservationV1 {
    let intent = wire::EffectIntentV1 {
        effect_id: id("synthetic-effect"),
        execution: ticket.execution.clone(),
        registration: ticket.registration.clone(),
        grant: ticket.basis.clone(),
        operation: wire::EffectOperationClass::ToolRequest,
        request_digest: digest('d'),
        provider_idempotency: None,
    };
    let permission = fixture
        .coordinator
        .prepare_effect(&ticket.execution.receiver, &intent)
        .await
        .unwrap();
    wire::EffectObservationV1 {
        protocol: 1,
        permission_id: permission.permission_id,
        execution: intent.execution,
        effect_id: intent.effect_id,
        sequence: generation(1),
        outcome: wire::EffectOutcome::CommitConfirmed,
        evidence_digest: digest('e'),
    }
}

async fn observe(fixture: &Fixture, observation: &wire::EffectObservationV1) {
    fixture
        .coordinator
        .observe_effect(&observation.execution.receiver, observation)
        .await
        .unwrap();
}

async fn retire(fixture: &Fixture, sandbox_id: &str) {
    let raw = fixture
        .sandbox
        .sandboxes
        .find_one(doc! {"_id":sandbox_id})
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
            doc! {"_id":sandbox_id},
            crate::store::sandboxes::sandbox_document(&sandbox, 0).unwrap(),
        )
        .await
        .unwrap();
}
