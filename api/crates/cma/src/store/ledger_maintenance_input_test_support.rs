use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxMutationGuard;
use layer2_core::sandbox::{Agent, Sandbox};

pub(super) fn reservation(agent: &Agent, parent: &Sandbox, key: &str) -> CommandReservation {
    let command = AgentCommand::message("Inspect retained fixture.");
    CommandReservation {
        binding: None,
        authority_deadline: None,
        admission_audit: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: layer2_core::ids::AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: AgentOwner::Person {
                subject: agent.owner.clone(),
            },
            actor: AgentActor::Human {
                subject: agent.owner.clone(),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "fixture-source".into(),
            },
            correlation_id: "fixture-correlation".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                authority_id: "fixture-authority".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(layer2_core::ids::AgentResponseId::generate()),
        admitted_at: NOW.into(),
        queue_deadline: NEXT.into(),
        queue_capacity: 2,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(parent),
        policy: layer2_core::resource_access::ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
    }
}

pub(super) fn claim_request(agent: &Agent, now: &str) -> CommandClaimRequest {
    CommandClaimRequest {
        agent_id: agent.id.clone(),
        holder: "fixture-command-worker".into(),
        now: now.into(),
        expires_at: NEXT.into(),
    }
}

/// Build a newly proposed admission only before any hold is accepted. Never
/// replace accepted bytes or refresh the source during operation replay.
pub(super) async fn refresh(store: &MongoSandboxStore, input: &mut LedgerPreExclusionAdmission) {
    let mut plan = input.accepted.plan().clone();
    let inspection = store
        .inspect_pre_exclusion(&input.caller, &plan.sandbox_id)
        .await
        .unwrap();
    assert!(inspection.state.is_none());
    plan.source = inspection.source;
    input.accepted =
        LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
}

/// Compare all fixture collections, including command, budget, audit and log
/// sidecars, so a refusal cannot pass after partially persisting another record.
pub(super) async fn all_rows(store: &MongoSandboxStore) -> Vec<(String, Vec<Document>)> {
    let database = store.client.database(&store.sandboxes.namespace().db);
    let mut names = database.list_collection_names().await.unwrap();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        let rows = database
            .collection::<Document>(&name)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        result.push((name, rows));
    }
    result
}

pub(super) async fn control_fixture(
    store: &MongoSandboxStore,
    delete: bool,
) -> (Policy, LedgerPreExclusionAdmission) {
    let (policy, mut input) = fixture(store).await;
    if delete {
        let mut agent = input.accepted.plan().source.agents[0].clone();
        let revision = agent.revision;
        agent
            .transition(
                AgentStatus::Stopped,
                NOW.into(),
                Some(AgentStopReason::Owner),
            )
            .unwrap();
        store.save_agent(revision, agent).await.unwrap();
        refresh(store, &mut input).await;
    }
    (policy, input)
}

pub(super) async fn control(
    store: &MongoSandboxStore,
    agent: &Agent,
    delete: bool,
) -> Result<AgentControlResult, AgentControlStoreError> {
    let target = AgentControlTarget::from(agent);
    if delete {
        store
            .schedule_agent_delete(AgentDeleteRequest {
                target,
                requested_at: NOW.into(),
            })
            .await
    } else {
        store
            .schedule_agent_stop(AgentStopRequest {
                target,
                reason: AgentStopReason::Owner,
                requested_at: NOW.into(),
            })
            .await
    }
}
