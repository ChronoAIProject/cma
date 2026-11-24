use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use layer2_core::{
    resource_access::{
        OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
    },
    sandbox::{
        store::{
            AgentLogStore, AgentStore, SandboxEventStore, SandboxLifecycleAgentScope,
            SandboxLifecycleExecutionStore, SandboxLifecycleMutation, SandboxMutationGuard,
            SandboxStore, SandboxWorkerStore, WorkspaceStore,
        },
        Agent, AgentLogEntry, AgentLogKind, AgentStatus, RetainedReopenError, RetainedReopenProof,
        RetainedReopenService, RetainedReopenVerifier, RetainedSuspendedReopenProof,
        SandboxEventKind, SandboxRevivePhase, SandboxStatus,
    },
    Authorization, Caller, Capability, SubjectId,
};

use super::*;
use crate::store::{
    agent_log::log_document, agents::agent_document,
    sandbox_maintenance::mongo_integration_tests::database,
};

const AT: &str = "2026-09-05T00:30:00Z";

#[path = "suspended_assessment_tests.rs"]
mod assessment;

#[path = "repeat_tests.rs"]
mod repeat;

async fn live(store: &MongoSandboxStore, id: &str, active: bool) -> Agent {
    let original = super::super::mongo_fixtures::reopen_fixture(store, id).await;
    let agent = original.snapshot.retained.agents[0].clone();
    let generation = original
        .snapshot
        .retained
        .sandbox
        .runtime
        .unwrap()
        .provision_generation
        .unwrap();
    set_live_child(store, agent, generation, active).await
}

async fn set_live_child(
    store: &MongoSandboxStore,
    mut agent: Agent,
    generation: u64,
    active: bool,
) -> Agent {
    agent.status = if active {
        AgentStatus::Working
    } else {
        AgentStatus::Ready
    };
    agent.conversation.runtime_generation = Some(generation);
    agent.conversation.active_turn = active.then_some(3);
    let mut events = vec![
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: generation,
        },
    ];
    if active {
        events.push(AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        });
        events.push(AgentLogKind::OwnerMessage {
            content: "retained input fixture".into(),
        });
    }
    agent.conversation.last_seq = events.len() as u64;
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap();
    store
        .agent_log
        .insert_many(events.into_iter().enumerate().map(|(i, kind)| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: i as u64 + 1,
                appended_at: AT.into(),
                kind,
            })
            .unwrap()
        }))
        .await
        .unwrap();
    agent
}

struct Verifier {
    calls: AtomicUsize,
    arrivals: Option<tokio::sync::Barrier>,
}

#[async_trait::async_trait]
impl RetainedReopenVerifier for Verifier {
    async fn verify(
        &self,
        _: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError> {
        panic!("suspended admission must not attempt idle reopening")
    }

    async fn verify_suspended(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedSuspendedReopenProof, SandboxStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(arrivals) = &self.arrivals {
            arrivals.wait().await;
        }
        proof(snapshot)
    }
}

fn proof(
    snapshot: &RetainedReopenSnapshot,
) -> Result<RetainedSuspendedReopenProof, SandboxStoreError> {
    let source = snapshot.suspended_source()?;
    Ok(RetainedSuspendedReopenProof {
        binding: source.binding.clone(),
        home_identity: source.home_identity,
    })
}

fn caller(owner: SubjectId) -> Caller {
    Caller::new(owner, Authorization::new([Capability::UseOwnSandboxes]))
}

async fn row(store: &MongoSandboxStore, id: &SandboxId) -> Document {
    store
        .sandboxes
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn suspended_admission_refuses_changed_membership_and_organization_ownership() {
    let (client, name, store) = database().await;
    let child = live(&store, "sbx_suspended_authority", false).await;
    let id = &child.sandbox_id;
    let snapshot = store.retained_suspended_reopen_snapshot(id).await.unwrap();
    let mut request = RetainedSuspendedReopenRequest {
        repeat_authorized: false,
        proof: proof(&snapshot).unwrap(),
        snapshot,
        key_sha256: "b".repeat(64),
        requested_at: AT.into(),
    };
    let mut workspace = store
        .get_workspace(&child.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.revision += 1;
    workspace.next_worktree_number += 1;
    store.save_workspace(revision, workspace).await.unwrap();
    assert!(matches!(
        store.admit_suspended_reopen(request.clone()).await,
        Err(SandboxStoreError::Conflict)
    ));
    let unchanged = row(&store, id).await;
    assert!(pair(&unchanged).unwrap().is_none());
    assert!(store.sandbox_events(id, 0, 10).await.unwrap().is_empty());
    assert_eq!(store.get_agent(&child.id).await.unwrap().unwrap(), child);

    request.snapshot = store.retained_suspended_reopen_snapshot(id).await.unwrap();
    let parent = &request.snapshot.retained.sandbox;
    let policy = ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: parent.owner.clone(),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "synthetic-issuer".into(),
            organization: parent.owner.clone(),
            admitted_by: ResourceActor::Human {
                subject: SubjectId::new("fixture-manager"),
            },
            verified_at: AT.into(),
        }),
        revision: 1,
    };
    let policy = crate::store::resource_access::records::policy_document(&policy).unwrap();
    store.resource_policies.insert_one(&policy).await.unwrap();
    assert!(matches!(
        store.retained_suspended_reopen_snapshot(id).await,
        Err(SandboxStoreError::NotFound)
    ));
    assert!(matches!(
        store.admit_suspended_reopen(request.clone()).await,
        Err(SandboxStoreError::NotFound)
    ));
    assert_eq!(row(&store, id).await, unchanged);

    store
        .resource_policies
        .delete_one(doc! {"_id":id.as_str()})
        .await
        .unwrap();
    store.admit_suspended_reopen(request).await.unwrap();
    let admitted = row(&store, id).await;
    assert!(pair(&admitted).unwrap().is_some());
    store.resource_policies.insert_one(policy).await.unwrap();
    assert!(matches!(
        store.retained_reopen_replay(id).await,
        Err(SandboxStoreError::NotFound)
    ));
    assert_eq!(row(&store, id).await, admitted);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn competing_consents_settle_once_and_flag_rollback_keeps_only_exact_replay() {
    let (client, name, store) = database().await;
    let child = live(&store, "sbx_suspended_once", true).await;
    let id = &child.sandbox_id;
    let parent = store.get_sandbox(id).await.unwrap().unwrap();
    let old = parent.runtime.clone().unwrap();
    let owner = caller(parent.owner.clone());
    let verifier = Arc::new(Verifier {
        calls: AtomicUsize::new(0),
        arrivals: Some(tokio::sync::Barrier::new(2)),
    });
    let first = Arc::new(store);
    let later = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let off = RetainedReopenService::new(later.clone(), verifier.clone(), false);
    assert!(matches!(
        off.request(&owner, id, parent.revision, "first-key", true)
            .await,
        Err(RetainedReopenError::Unavailable)
    ));
    let a = RetainedReopenService::new(first.clone(), verifier.clone(), false)
        .with_suspended_admission(true);
    let b =
        RetainedReopenService::new(later, verifier.clone(), false).with_suspended_admission(true);
    let (a, b) = tokio::join!(
        a.request(&owner, id, parent.revision, "first-key", true),
        b.request(&owner, id, parent.revision, "second-key", true)
    );
    let (accepted, key) = match (a, b) {
        (Ok(accepted), Err(RetainedReopenError::Conflict)) => (accepted, "first-key"),
        (Err(RetainedReopenError::Conflict), Ok(accepted)) => (accepted, "second-key"),
        other => panic!("exactly one consent must commit: {other:?}"),
    };
    let original = row(&first, id).await;
    assert_eq!(pair(&original).unwrap().unwrap().accepted, accepted);
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
    assert_eq!(first.sandbox_events(id, 0, 20).await.unwrap().len(), 1);
    let admitted = first.get_sandbox(id).await.unwrap().unwrap();
    assert_eq!(admitted.status, SandboxStatus::Reviving);
    assert!(admitted.runtime.is_none());
    assert_eq!(admitted.lifecycle.suspended_runtime.as_deref(), Some(&old));
    let current = first.get_agent(&child.id).await.unwrap().unwrap();
    assert_eq!(current.status, AgentStatus::Paused);
    assert_eq!(
        current.conversation.conversation_id,
        child.conversation.conversation_id
    );
    assert!(current.conversation.active_turn.is_none());
    let logs = first.agent_log(&child.id, 0, 25).await.unwrap();
    assert!(logs
        .iter()
        .any(|row| matches!(row.kind, AgentLogKind::TurnInterrupted { turn_sequence: 3 })));
    assert!(logs.iter().any(|row| matches!(&row.kind,AgentLogKind::OwnerMessage {content} if content=="retained input fixture")));
    assert_eq!(
        off.request(&owner, id, parent.revision, key, true)
            .await
            .unwrap(),
        accepted
    );
    assert!(matches!(
        off.request(&owner, id, parent.revision + 1, key, true)
            .await,
        Err(RetainedReopenError::Conflict)
    ));
    assert!(matches!(
        off.request(&owner, id, parent.revision, "fresh-key", true)
            .await,
        Err(RetainedReopenError::StaleRequest)
    ));
    assert!(matches!(
        off.request(
            &caller(SubjectId::new("foreign-fixture")),
            id,
            parent.revision,
            key,
            true
        )
        .await,
        Err(RetainedReopenError::NotFound)
    ));
    assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
    assert_eq!(row(&first, id).await, original);
    client.database(&name).drop().await.unwrap();
}

async fn save_failed_revival(store: &MongoSandboxStore, id: &SandboxId) {
    let mut bound = store.get_sandbox(id).await.unwrap().unwrap();
    let observed_at = bound.updated_at.clone();
    // Present the already installed synthetic replacement at its restore phase.
    // The actual terminal state and event go through the ordinary claimed save.
    let mut replacement = *bound.lifecycle.suspended_runtime.take().unwrap();
    replacement.provision_generation = Some(4);
    replacement.boot_id = Some("c".repeat(32));
    bound.runtime = Some(replacement);
    bound.lifecycle.runtime_generation_high_water = 4;
    let Some(SandboxLifecycleOperation::Revive { phase, .. }) = bound.lifecycle.operation.as_mut()
    else {
        panic!("revive intent is required")
    };
    *phase = SandboxRevivePhase::ConversationReopen;
    let revision = bound.revision;
    bound.revision += 1;
    bound.validate().unwrap();
    store.save_sandbox(revision, bound).await.unwrap();
    let claimed = store
        .claim_sandbox_work(
            id,
            "fixture-worker",
            observed_at.clone(),
            crate::store::sandbox_mongo_integration_tests::work_lease_deadline(),
        )
        .await
        .unwrap()
        .unwrap();
    let mut failed = claimed.clone();
    let lease = claimed.work.lease.as_ref().unwrap();
    let failure = failed
        .fail_lifecycle(
            "synthetic failed installation".into(),
            &lease.holder,
            lease.fence,
            observed_at.clone(),
        )
        .unwrap();
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&claimed),
            sandbox: failed,
            agents: Vec::new(),
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: observed_at,
            events: vec![SandboxEventKind::Failed { failure }],
        })
        .await
        .unwrap();
}

fn legacy_request(snapshot: RetainedReopenSnapshot, key: String) -> RetainedReopenRequest {
    let plan = serde_json::json!({"synthetic":"idle-reopen"});
    let source = snapshot.retained.source().unwrap();
    let proof = RetainedReopenProof {
        expected_runtime: source.binding.clone(),
        observed_boot: "d".repeat(32),
        home_identity: source.home_identity,
        provider_plan_sha256:
            layer2_core::sandbox::SandboxUnpublishedRecoveryProof::provider_plan_digest(&plan)
                .unwrap(),
        provider_plan: plan,
        histories: snapshot
            .retained
            .agents
            .iter()
            .map(|agent| layer3_adapter::RecoveryConversationHistory {
                agent_id: agent.id.to_string(),
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                history_sha256: "e".repeat(64),
            })
            .collect(),
    };
    proof.validate(&snapshot).unwrap();
    RetainedReopenRequest {
        repeat_authorized: false,
        snapshot,
        proof,
        key_sha256: key,
        requested_at: AT.into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn suspended_receipt_pair_survives_failed_revive_and_reconciliation() {
    let (client, name, store) = database().await;
    let child = live(&store, "sbx_suspended_predecessor", false).await;
    let id = &child.sandbox_id;
    let snapshot = store.retained_suspended_reopen_snapshot(id).await.unwrap();
    let request = RetainedSuspendedReopenRequest {
        repeat_authorized: false,
        proof: proof(&snapshot).unwrap(),
        snapshot,
        key_sha256: "a".repeat(64),
        requested_at: AT.into(),
    };
    let mut session = client.start_session().await.unwrap();
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let Applied::Commit(receipt, event, repeat) = apply(&store, &request, scope).await.unwrap()
    else {
        panic!("expected atomic settlement")
    };
    session.commit_transaction().await.unwrap();
    let time = session.operation_time().unwrap();
    assert!(!repeat);
    let committed = row(&store, id).await;
    assert_eq!(pair(&committed).unwrap().as_ref(), Some(receipt.as_ref()));

    for field in [FIELD, SUSPENDED_FIELD] {
        let mut half = committed.clone();
        half.remove(field);
        assert!(matches!(pair(&half), Err(SandboxStoreError::Corrupt(_))));
    }
    let mut conflicting = committed.clone();
    conflicting
        .get_document_mut(FIELD)
        .unwrap()
        .insert("receipt_sha256", "0".repeat(64));
    assert!(matches!(
        pair(&conflicting),
        Err(SandboxStoreError::Corrupt(_))
    ));

    save_failed_revival(&store, id).await;
    let failed = row(&store, id).await;
    assert_eq!(read_sandbox(&failed).unwrap().status, SandboxStatus::Failed);
    for field in [FIELD, SUSPENDED_FIELD] {
        assert_eq!(failed.get(field), committed.get(field));
    }
    reconcile(&store, &receipt, &event, time, repeat)
        .await
        .unwrap();
    let exact = store.admit_suspended_reopen(request.clone()).await.unwrap();
    assert_eq!(exact, receipt.accepted);
    // The predecessor's ordinary reader still refuses the suspended marker.
    assert!(matches!(
        transaction::read_receipt(&failed),
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(row(&store, id).await, failed);
    assert_eq!(store.sandbox_events(id, 0, 10).await.unwrap().len(), 2);
    let control =
        super::super::mongo_fixtures::reopen_fixture(&store, "sbx_unmarked_control").await;
    assert!(store.admit_retained_reopen(control).await.is_ok());
    client.database(&name).drop().await.unwrap();
}
