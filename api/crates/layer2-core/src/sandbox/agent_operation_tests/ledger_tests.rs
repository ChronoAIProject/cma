use std::sync::Arc;

use super::fixtures::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentStore, SandboxMutationGuard, SandboxStore};
use crate::sandbox::*;

pub(super) fn reservation(agent: &Agent, parent: &Sandbox, key: &str) -> CommandReservation {
    let command = AgentCommand::message("Inspect this.");
    CommandReservation {
        binding: None,
        authority_deadline: None,
        admission_audit: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: AgentOwner::Person {
                subject: agent.owner.clone(),
            },
            actor: AgentActor::Human {
                subject: agent.owner.clone(),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "interaction-one".into(),
            },
            correlation_id: "correlation-one".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal-owner".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(AgentResponseId::generate()),
        admitted_at: NOW.into(),
        queue_deadline: LATEST.into(),
        queue_capacity: 2,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(parent),
        policy: crate::resource_access::ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
    }
}

async fn fixture() -> (Arc<InMemorySandboxStore>, Agent, Sandbox) {
    let store = Arc::new(InMemorySandboxStore::default());
    let parent = seed_active_parent(&store).await;
    let agent = test_agent(AgentStatus::Working, None);
    store.create_agent(agent.clone()).await.unwrap();
    (store, agent, parent)
}

pub(super) fn record(result: CommandReservationResult) -> AdmittedAgentCommand {
    match result {
        CommandReservationResult::Created(value) | CommandReservationResult::Existing(value) => {
            *value
        }
    }
}

#[tokio::test]
async fn racing_retries_reserve_one_input_response_and_order() {
    let (store, agent, parent) = fixture().await;
    let a = reservation(&agent, &parent, "request-one");
    let mut b = reservation(&agent, &parent, "request-one");
    b.envelope.authorization.policy_revision = 2;
    b.envelope.source.kind = AgentSourceKind::Api;
    let (one, two) = tokio::join!(store.reserve_command(a), store.reserve_command(b.clone()));
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_ne!(
        matches!(one, CommandReservationResult::Created(_)),
        matches!(two, CommandReservationResult::Created(_))
    );
    let original = record(one);
    assert_eq!(original, record(two));
    assert_eq!(original.receipt.agent_order, 1);
    assert_eq!(
        store
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(original.clone())
    );
    b.envelope.command = AgentCommand::message("Different input");
    b.envelope.request_fingerprint = b.envelope.command.fingerprint().unwrap();
    assert_eq!(
        store.reserve_command(b).await.unwrap_err(),
        CommandLedgerError::IdempotencyConflict
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn independent_authorities_share_one_bounded_order_and_retries_keep_their_slot() {
    let (store, agent, parent) = fixture().await;
    let first = reservation(&agent, &parent, "request-one");
    let first_record = record(store.reserve_command(first.clone()).await.unwrap());
    let mut second = reservation(&agent, &parent, "request-one");
    second.envelope.authorization.authority_id = "another-verified-grant".into();
    second.envelope.source.kind = AgentSourceKind::Api;
    second.envelope.actor = AgentActor::Api {
        subject: agent.owner.clone(),
        delegation_id: None,
    };
    let second_record = record(store.reserve_command(second).await.unwrap());
    assert_eq!(second_record.receipt.agent_order, 2);
    assert_eq!(
        store
            .reserve_command(reservation(&agent, &parent, "request-three"))
            .await
            .unwrap_err(),
        CommandLedgerError::QueueFull
    );
    assert_eq!(
        record(store.reserve_command(first).await.unwrap()),
        first_record
    );
}

#[tokio::test]
async fn stale_or_foreign_admission_has_no_receipt_or_consumed_order() {
    let (store, agent, parent) = fixture().await;
    let mut stale = reservation(&agent, &parent, "request-one");
    stale.expected_agent_revision += 1;
    let id = stale.envelope.command_id.clone();
    assert_eq!(
        store.reserve_command(stale).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert!(store.get_command(&id).await.unwrap().is_none());
    let mut foreign = reservation(&agent, &parent, "request-one");
    foreign.envelope.owner = AgentOwner::Person {
        subject: crate::SubjectId::new("someone-else"),
    };
    assert_eq!(
        store.reserve_command(foreign).await.unwrap_err(),
        CommandLedgerError::NotFound
    );
    let valid = record(
        store
            .reserve_command(reservation(&agent, &parent, "request-one"))
            .await
            .unwrap(),
    );
    assert_eq!(valid.receipt.agent_order, 1);
}

#[tokio::test]
async fn retry_without_matching_task_evidence_leaves_the_command_order_available() {
    use crate::sandbox::store::AgentLogStore;
    use crate::sandbox::task_acceptance::{TaskAcceptanceAction, TaskAcceptanceIntent};

    let (store, agent, parent) = fixture().await;
    let original_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
    let ordinary = reservation(&agent, &parent, "task-retry");
    let mut retry = ordinary.clone();
    let AgentCommand::Message { input, .. } = &mut retry.envelope.command else {
        unreachable!();
    };
    input.task_acceptance = Some(TaskAcceptanceIntent {
        version: 1,
        action: TaskAcceptanceAction::Retry {
            task_id: AgentCommandId::generate(),
            after_turn_sequence: 1,
        },
    });
    retry.envelope.request_fingerprint = retry.envelope.command.fingerprint().unwrap();

    assert_eq!(
        store.reserve_command(retry).await,
        Err(CommandLedgerError::NotAdmissible)
    );
    assert!(store
        .get_command(&ordinary.envelope.command_id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.agent_log(&agent.id, 0, 20).await.unwrap(),
        original_log
    );

    let CommandReservationResult::Created(admitted) =
        store.reserve_command(ordinary).await.unwrap()
    else {
        panic!("the refused retry must leave its key available");
    };
    assert_eq!(admitted.receipt.agent_order, 1);
}

fn personal_policy(parent: &Sandbox) -> crate::resource_access::ResourcePolicyRecord {
    crate::resource_access::ResourcePolicyRecord {
        sandbox_id: parent.id.clone(),
        sandbox_created_at: parent.created_at.clone(),
        owner: AgentOwner::Person {
            subject: parent.owner.clone(),
        },
        organization_evidence: None,
        revision: 1,
    }
}

#[tokio::test]
async fn terminal_agent_refuses_new_input_even_when_the_observation_predates_failed_open() {
    let agent = test_agent(AgentStatus::Starting, Some("Wait for input"));
    let store = InMemorySandboxStore::default();
    let parent = seed_active_parent(&store).await;
    let request = reservation(&agent, &parent, "never-accepted");
    let mut failed = agent.clone();
    failed
        .fail(
            TypedFailure {
                class: "agent_runtime_unsupported".into(),
                detail: "open failed".into(),
            },
            LATER.into(),
        )
        .unwrap();
    assert_eq!(
        validate_command_reservation(&request, &failed, &parent),
        Err(CommandLedgerError::NotAdmissible)
    );
    let mut foreign = request;
    foreign.envelope.owner = AgentOwner::Person {
        subject: crate::SubjectId::new("foreign"),
    };
    assert_eq!(
        validate_command_reservation(&foreign, &failed, &parent),
        Err(CommandLedgerError::NotFound)
    );
}

#[tokio::test]
async fn policy_revision_is_fenced_without_changing_aggregate_revisions_and_retries_survive() {
    let (store, agent, parent) = fixture().await;
    let original_request = reservation(&agent, &parent, "old-policy");
    let original = record(
        store
            .reserve_command(original_request.clone())
            .await
            .unwrap(),
    );
    store.set_resource_policy_for_test(personal_policy(&parent));
    let mut current = reservation(&agent, &parent, "new-policy");
    assert_eq!(
        store.reserve_command(current.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(
        record(store.reserve_command(original_request).await.unwrap()),
        original
    );
    current.policy.policy_revision = 1;
    // Credential custody revision is independent of the resource policy fence.
    current.envelope.authorization.policy_revision = 17;
    let admitted = record(store.reserve_command(current).await.unwrap());
    assert_eq!(admitted.receipt.agent_order, 2);
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
}

#[tokio::test]
async fn owner_kind_incarnation_and_corrupt_policy_cannot_admit_or_consume_order() {
    use crate::resource_access::*;
    let (store, agent, parent) = fixture().await;
    let mut submission = reservation(&agent, &parent, "fenced");
    submission.policy.policy_revision = 1;
    let mut policy = personal_policy(&parent);
    policy.owner = ResourceOwner::Organization {
        subject: parent.owner.clone(),
    };
    policy.organization_evidence = Some(OrganizationOwnershipEvidence {
        issuer: "https://identity.test".into(),
        organization: parent.owner.clone(),
        admitted_by: ResourceActor::Human {
            subject: crate::SubjectId::new("admin"),
        },
        verified_at: NOW.into(),
    });
    store.set_resource_policy_for_test(policy);
    assert_eq!(
        store.reserve_command(submission.clone()).await,
        Err(CommandLedgerError::NotFound)
    );
    for corruption in 0..3 {
        let mut policy = personal_policy(&parent);
        match corruption {
            0 => policy.sandbox_created_at = LATEST.into(),
            1 => policy.revision = 0,
            _ => {
                policy.owner = ResourceOwner::Person {
                    subject: crate::SubjectId::new("foreign"),
                }
            }
        }
        store.set_resource_policy_for_test(policy);
        assert_eq!(
            store.reserve_command(submission.clone()).await,
            Err(CommandLedgerError::Corrupt)
        );
    }
    store.set_resource_policy_for_test(personal_policy(&parent));
    submission.policy.sandbox_created_at = LATEST.into();
    assert_eq!(
        store.reserve_command(submission.clone()).await,
        Err(CommandLedgerError::Conflict)
    );
    submission.policy.sandbox_created_at = parent.created_at.clone();
    assert_eq!(
        record(store.reserve_command(submission).await.unwrap())
            .receipt
            .agent_order,
        1
    );
}

#[tokio::test(start_paused = true)]
async fn expired_membership_cannot_create_or_recover_a_reservation() {
    let (store, agent, parent) = fixture().await;
    let original_request = reservation(&agent, &parent, "retained");
    let original = record(
        store
            .reserve_command(original_request.clone())
            .await
            .unwrap(),
    );
    for mut submission in [original_request, reservation(&agent, &parent, "expired")] {
        submission.authority_deadline = Some(tokio::time::Instant::now());
        assert_eq!(
            store.reserve_command(submission).await,
            Err(CommandLedgerError::Conflict)
        );
    }
    assert_eq!(
        store
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(original)
    );
    assert_eq!(
        record(
            store
                .reserve_command(reservation(&agent, &parent, "fresh"))
                .await
                .unwrap()
        )
        .receipt
        .agent_order,
        2
    );
}

#[tokio::test]
async fn response_identity_is_unique_and_duplicate_refusal_does_not_consume_order() {
    let (store, agent, parent) = fixture().await;
    let first = record(
        store
            .reserve_command(reservation(&agent, &parent, "first"))
            .await
            .unwrap(),
    );
    let response_id = first.receipt.response_id.as_ref().unwrap();
    assert_eq!(
        store.get_response_command(response_id).await.unwrap(),
        Some(first.clone())
    );
    let mut duplicate = reservation(&agent, &parent, "second");
    duplicate.response_id = Some(response_id.clone());
    assert_eq!(
        store.reserve_command(duplicate).await,
        Err(CommandLedgerError::Conflict)
    );
    assert!(store
        .get_response_command(&AgentResponseId::generate())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        record(
            store
                .reserve_command(reservation(&agent, &parent, "third"))
                .await
                .unwrap()
        )
        .receipt
        .agent_order,
        2
    );
}
