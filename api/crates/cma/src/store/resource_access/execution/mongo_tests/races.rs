use super::*;
use fixture::{context, Fixture};
use layer2_core::resource_access::{AccessPurpose, ResourceCapability, ResourceShareStore};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn registration_before_freeze_is_retained_until_every_exact_ack() {
    let fixture = Fixture::new().await;
    let first = fixture.register("recipient-one", "first").await;
    let second = fixture.register("recipient-two", "second").await;
    let owner = fixture.register("owner", "independent-owner").await;
    let operation = fixture.freeze().await;
    assert_eq!(operation.frozen_members, 2);
    assert_eq!(first.basis.family(), second.basis.family());
    assert!(owner.basis.family().is_none());
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    fixture.complete(&operation, &first).await;
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    fixture.complete(&operation, &second).await;
    fixture.complete(&operation, &second).await;
    let effective = fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    let RevocationPhaseV1::Effective { receipt } = &effective.phase else {
        panic!("missing effective receipt")
    };
    assert!(receipt.share.revoked_at.is_some());
    let replay = fixture
        .coordinator
        .begin_revocation(&context("owner"), None, fixture.command("freeze"))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(effective).unwrap(),
        serde_json::to_value(replay).unwrap()
    );
    assert!(fixture
        .sandbox
        .resource_share(&fixture.share.id)
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    fixture.register("owner", "owner-after-freeze").await;
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn freeze_wins_against_registration_with_an_older_mongo_snapshot() {
    let fixture = Fixture::new().await;
    let authority = fixture.authority("recipient").await;
    let member = wire::MemberKey::Execution {
        execution: fixture.execution("delayed"),
    };
    let mut session = fixture.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    fixture
        .store
        .collections
        .grants
        .find_one(doc! {"_id":authority.basis().family().unwrap().origin.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    let operation = fixture.freeze().await;
    assert_eq!(operation.frozen_members, 0);
    let result = fixture
        .store
        .register_member(&authority, &member, &mut session)
        .await;
    assert!(matches!(result, Err(TxnError::Driver(ref error)) if is_transient(error)));
    session.abort_transaction().await.unwrap();
    assert!(matches!(
        fixture.store.register(&authority, &member).await,
        Err(ExecutionError::Closing { .. })
    ));
    assert!(fixture
        .store
        .record(&registration_id(&member).unwrap())
        .await
        .unwrap()
        .is_none());
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn closing_denies_cached_reads_and_legacy_mutation_bypass() {
    let fixture = Fixture::new().await;
    let before = fixture
        .access
        .authorize(
            &context("recipient"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::ViewHistory,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    let operation = fixture.freeze().await;
    assert!(fixture
        .access
        .validate_read_result(&before, &fixture.target, ResourceCapability::ViewHistory)
        .await
        .is_err());
    assert!(fixture
        .access
        .authorize(
            &context("recipient"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::ViewHistory,
            AccessPurpose::Read
        )
        .await
        .is_err());
    let raw = operation
        .intent
        .mutation(operation.intent.original_policy_revision);
    assert!(fixture.sandbox.mutate_share(raw).await.is_err());
    fixture
        .access
        .authorize(
            &context("owner"),
            None,
            &fixture.target,
            ResourceCapability::ViewHistory,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn link_redemption_cannot_change_the_revision_of_a_frozen_grant() {
    use layer2_core::resource_access::{
        AgentShareAudience, ResourceAccessStore, ResourceActor, ResourceOwner, ResourceSelector,
        ShareChange, ShareCommand, ShareMode, ShareMutation,
    };
    use layer2_core::SubjectId;
    let fixture = Fixture::new().await;
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    let selector = ResourceSelector::Agent {
        agent_id: snapshot.lineage.agent.as_ref().unwrap().id.clone(),
    };
    let request = ShareMutation {
        selector: selector.clone(),
        owner: ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        key_sha256: fixture::digest('b').as_str().into(),
        change: ShareChange::CreateLink {
            audience: AgentShareAudience::Public,
            mode: ShareMode::View,
            token_sha256: fixture::digest('c').as_str().into(),
        },
        expected_share_revision: None,
        policy_fence: snapshot.fence(),
        proposed_id: "synthetic-link".into(),
        at: "2026-01-01T00:00:00Z".into(),
    };
    let created = fixture.sandbox.mutate_share(request.clone()).await.unwrap();
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    let mut redeem = ShareMutation {
        actor: ResourceActor::Human {
            subject: SubjectId::new("recipient-one"),
        },
        key_sha256: fixture::digest('d').as_str().into(),
        change: ShareChange::RedeemLink {
            share_id: created.share.id.clone(),
            token_sha256: fixture::digest('c').as_str().into(),
            subject: SubjectId::new("recipient-one"),
        },
        expected_share_revision: Some(created.share.revision),
        policy_fence: snapshot.fence(),
        ..request
    };
    let accepted = fixture.sandbox.mutate_share(redeem.clone()).await.unwrap();
    let operation = fixture
        .coordinator
        .begin_revocation(
            &context("owner"),
            None,
            ShareCommand {
                selector,
                change: ShareChange::Revoke {
                    share_id: accepted.share.id.clone(),
                },
                expected_share_revision: Some(accepted.share.revision),
                idempotency_key: "freeze-link".into(),
            },
        )
        .await
        .unwrap();
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    redeem.actor = ResourceActor::Human {
        subject: SubjectId::new("recipient-two"),
    };
    redeem.key_sha256 = fixture::digest('e').as_str().into();
    redeem.change = ShareChange::RedeemLink {
        share_id: accepted.share.id.clone(),
        token_sha256: fixture::digest('c').as_str().into(),
        subject: SubjectId::new("recipient-two"),
    };
    redeem.expected_share_revision = Some(accepted.share.revision);
    redeem.policy_fence = snapshot.fence();
    assert!(matches!(
        fixture.sandbox.mutate_share(redeem).await,
        Err(layer2_core::resource_access::ResourceAccessStoreError::Conflict)
    ));
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_family_freezes_two_agents_and_old_new_policy_observations() {
    use layer2_core::resource_access::{
        ResourceAccessStore, ResourceTarget, ShareChange, ShareMode, ShareMutation,
    };
    use layer2_core::sandbox::store::WorktreeStore;
    use layer2_core::{AgentId, SubjectId, WorktreeId};
    let fixture = Fixture::new().await;
    let first = fixture.register("recipient-one", "agent-one").await;
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    let mut worktree = snapshot.lineage.worktree.clone().unwrap();
    let mut agent = snapshot.lineage.agent.clone().unwrap();
    agent.id = AgentId("synthetic-agent-two".into());
    agent.creation_key = "create-synthetic-agent-two".into();
    worktree.id = WorktreeId("synthetic-worktree-two".into());
    worktree.agent_id = Some(agent.id.clone());
    worktree.name = "second".into();
    worktree.path = "/synthetic/second".into();
    agent.worktree_id = worktree.id.clone();
    fixture.sandbox.create_worktree(worktree).await.unwrap();
    fixture
        .sandbox
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    let other = ShareMutation {
        selector: fixture.selector.clone(),
        owner: fixture.share.owner.clone(),
        actor: fixture.share.issuer_actor.clone(),
        key_sha256: fixture::digest('f').as_str().into(),
        change: ShareChange::Create {
            target_org_id: SubjectId::new("another-org"),
            mode: ShareMode::OrgRoles,
        },
        expected_share_revision: None,
        policy_fence: snapshot.fence(),
        proposed_id: "zz-independent-grant".into(),
        at: "2026-01-01T00:00:01Z".into(),
    };
    fixture.sandbox.mutate_share(other).await.unwrap();
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    let authority = fixture
        .coordinator
        .authorize_execution(
            &context("recipient-two"),
            Some("synthetic"),
            &target,
            ResourceCapability::Interact,
            None,
        )
        .await
        .unwrap();
    let mut key = fixture.execution("agent-two");
    key.agent_id = fixture::id(agent.id.as_str());
    let second = fixture
        .coordinator
        .register_execution(&authority, &key)
        .await
        .unwrap();
    let third = fixture.register("recipient-three", "third").await;
    assert_eq!(first.basis.family(), second.basis.family());
    assert_eq!(second.basis.family(), third.basis.family());
    let (
        wire::ExecutionBasisV1::Share { grant: old },
        wire::ExecutionBasisV1::Share { grant: new },
    ) = (&first.basis, &second.basis)
    else {
        panic!()
    };
    assert!(old.policy_revision < new.policy_revision);
    assert_ne!(old.target_ancestry, new.target_ancestry);
    assert_ne!(old.actor, new.actor);
    let operation = fixture.freeze().await;
    assert_eq!(operation.frozen_members, 3);
    let members = fixture
        .store
        .frozen_members(&operation.key.operation_id, 0, 32)
        .await
        .unwrap();
    assert_eq!(members.len(), 3);
    let other = fixture.authority("recipient-one").await;
    assert_ne!(other.basis().family(), first.basis.family());
    assert!(fixture
        .coordinator
        .authorize_execution(
            &context("recipient-one"),
            Some("synthetic"),
            &fixture.target,
            ResourceCapability::Interact,
            Some(&first.basis),
        )
        .await
        .is_err());
    for member in [&first, &second, &third] {
        fixture.complete(&operation, member).await;
    }
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_effect_ledger_preserves_revoke_capacity_and_evidence_only_reconciliation() {
    let fixture = Fixture::new().await;
    let ticket = fixture.started_ticket("effects").await;
    let receiver = &ticket.execution.receiver;
    let make_intent = |index| wire::EffectIntentV1 {
        effect_id: fixture::id(&format!("effect-{index}")),
        execution: ticket.execution.clone(),
        registration: ticket.registration.clone(),
        grant: ticket.basis.clone(),
        operation: wire::EffectOperationClass::ToolRequest,
        request_digest: fixture::digest('b'),
        provider_idempotency: None,
    };
    let first = fixture
        .coordinator
        .prepare_effect(receiver, &make_intent(0))
        .await
        .unwrap();
    for index in 1..MAX_MEMBER_EFFECTS {
        fixture
            .coordinator
            .prepare_effect(receiver, &make_intent(index))
            .await
            .unwrap();
    }
    assert!(matches!(
        fixture
            .coordinator
            .prepare_effect(receiver, &make_intent(MAX_MEMBER_EFFECTS))
            .await,
        Err(ExecutionError::Capacity)
    ));
    let operation = fixture.freeze().await;
    assert!(matches!(
        fixture
            .coordinator
            .prepare_effect(receiver, &make_intent(0))
            .await,
        Err(ExecutionError::Closing { .. })
    ));
    let unknown = wire::EffectObservationV1 {
        protocol: 1,
        permission_id: first.permission_id.clone(),
        execution: ticket.execution.clone(),
        effect_id: first.intent.effect_id.clone(),
        sequence: fixture::generation(1),
        outcome: wire::EffectOutcome::SubmissionUnknown,
        evidence_digest: fixture::digest('c'),
    };
    fixture
        .coordinator
        .observe_effect(receiver, &unknown)
        .await
        .unwrap();
    let mut pending = wire::EffectObservationV1 {
        outcome: wire::EffectOutcome::ReconciliationRequired,
        ..unknown.clone()
    };
    for sequence in 2..MAX_EFFECT_OBSERVATIONS {
        pending.sequence = fixture::generation(sequence as u64);
        fixture.store.observe_effect(&pending).await.unwrap();
    }
    pending.sequence = fixture::generation(MAX_EFFECT_OBSERVATIONS as u64);
    assert!(matches!(
        fixture.store.observe_effect(&pending).await,
        Err(ExecutionError::Capacity)
    ));
    let member = fixture
        .store
        .record(&ticket.registration)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member.effect_count, MAX_MEMBER_EFFECTS);
    fixture.complete(&operation, &member).await;
    fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .unwrap();
    let confirmed = wire::EffectObservationV1 {
        sequence: fixture::generation(MAX_EFFECT_OBSERVATIONS as u64),
        outcome: wire::EffectOutcome::CommitConfirmed,
        ..unknown.clone()
    };
    fixture
        .coordinator
        .observe_effect(receiver, &confirmed)
        .await
        .unwrap();
    fixture
        .coordinator
        .observe_effect(receiver, &confirmed)
        .await
        .unwrap();
    let reopened = fixture
        .sandbox
        .open_resource_execution(false)
        .await
        .unwrap();
    reopened.observe_effect(&unknown).await.unwrap();
    let effects = reopened
        .effects_for(&ticket.registration, 0, 32)
        .await
        .unwrap();
    assert_eq!(effects[0].observations.len(), MAX_EFFECT_OBSERVATIONS);
    assert_eq!(effects[0].observations.first(), Some(&unknown));
    assert_eq!(effects[0].observations.last(), Some(&confirmed));
    assert_eq!(
        effects[0].replay_prepared(&first.intent),
        Err(ExecutionError::Unknown)
    );
    fixture.cleanup().await;
}
