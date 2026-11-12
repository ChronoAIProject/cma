use super::*;
use layer2_core::sandbox::store::SandboxMutationGuard;

async fn prepared(f: &Fixture, request: &AgentAdmission) -> Agent {
    let mut workspace = f
        .resources
        .get_workspace(&f.workspace.id)
        .await
        .unwrap()
        .unwrap();
    workspace.profile.content.agent_profiles = vec![layer2_core::profiles::AgentProfileRef::new(
        request.profile.id.clone(),
        request.profile.revision,
    )];
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    let revision = workspace.revision;
    workspace.revision += 1;
    f.resources
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    let mut automatic = request.clone();
    automatic.agent_id = layer2_core::AgentId::generate();
    automatic.worktree_id = WorktreeId::generate();
    automatic.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    automatic.created_by = None;
    automatic.first_message = None;
    let mut agent = f
        .resources
        .admit_agent(automatic)
        .await
        .unwrap()
        .creation()
        .agent
        .clone();
    agent.work.warm_conversation_id = Some("unused-prepared-session".into());
    persist(f, &agent).await;
    agent
}

async fn persist(f: &Fixture, agent: &Agent) {
    f.resources
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(agent).unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_prepared_reuse_preserves_original_agent_creation_key() {
    let (f, launch, request) = pending().await;
    let automatic = prepared(&f, &request).await;
    let result = f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await
        .unwrap();
    let agent = &result.creation().agent;
    assert_eq!(agent.id, automatic.id);
    assert_eq!(agent.origin, automatic.origin);
    let first = agent.conversation.first_message.as_ref().unwrap();
    assert_eq!(first.origin_key, automatic.creation_key);
    assert_ne!(first.origin_key, request.creation_key);
    assert_eq!(
        first.common_binding.as_ref().unwrap().receipt().agent_order,
        1
    );
    let replay = f
        .resources
        .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(replay.creation().agent, *agent);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_reuse_with_owner_input_or_accepted_command_adds_no_prompt() {
    for queued in [false, true] {
        let (f, launch, request) = pending().await;
        let mut agent = prepared(&f, &request).await;
        if queued {
            reserve_human(&f, &agent, "existing-queued-input").await;
        } else {
            agent.conversation.last_seq += 1;
            let log = AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: agent.conversation.last_seq,
                appended_at: request.event_at.clone(),
                kind: AgentLogKind::OwnerMessage {
                    content: "Existing owner input".into(),
                },
            };
            f.resources
                .agent_log
                .insert_one(crate::store::agent_log::log_document(&log).unwrap())
                .await
                .unwrap();
            persist(&f, &agent).await;
        }
        let result = f
            .resources
            .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), true)
            .await
            .unwrap();
        assert_eq!(result.creation().agent.id, agent.id);
        assert!(result.creation().agent.conversation.first_message.is_none());
        assert_eq!(
            f.resources
                .agent_commands
                .count_documents(doc! {"agent_id":agent.id.as_str()})
                .await
                .unwrap(),
            u64::from(queued)
        );
        let durable = f
            .seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap();
        assert!(durable.input_binding_sha256.is_none());
        f.close().await;
    }
}

pub(super) async fn reserve_human(f: &Fixture, agent: &Agent, key: &str) -> AdmittedAgentCommand {
    use layer2_core::resource_access::{ResourceOwner, ResourcePolicyFence};
    use layer2_core::sandbox::store::SandboxStore;
    let parent = f
        .resources
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let command = AgentCommand::message("Subsequent owner input");
    let request = CommandReservation {
        binding: None,
        nyxid_trace: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: layer2_core::ids::AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: ResourceOwner::Person {
                subject: agent.owner.clone(),
            },
            actor: AgentActor::Human {
                subject: agent.owner.clone(),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::UserInterface,
                reference: "test-owner-input".into(),
            },
            correlation_id: key.into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                authority_id: "test-owner-authority".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(layer2_core::ids::AgentResponseId::generate()),
        admitted_at: layer2_core::now_rfc3339(),
        queue_deadline: expiry(),
        queue_capacity: MAX_COMMAND_QUEUE_CAPACITY,
        expected_agent_revision: agent.revision,
        parent: SandboxMutationGuard::capture(&parent),
        policy: ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 0,
        },
        authority_deadline: None,
        admission_audit: None,
    };
    match f.resources.reserve_command(request).await.unwrap() {
        CommandReservationResult::Created(command)
        | CommandReservationResult::Existing(command) => *command,
    }
}
