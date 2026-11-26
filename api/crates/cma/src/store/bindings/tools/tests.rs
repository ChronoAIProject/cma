use super::*;
use crate::store::bindings::{admission_tests, mongo_tests};
use base64::Engine as _;
use layer2_core::profiles::{content_digest, AgentProfileSnapshot};
use layer2_core::sandbox::binding::{
    BindingAdmissionFence, BindingConversationState, BindingState, BindingWork,
};
use layer2_core::sandbox::store::*;
use layer2_core::sandbox::*;
use layer2_core::{AgentProfileId, RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::hosted::HostedAccessToolSource;

pub(super) async fn setup(
    store: &MongoSandboxStore,
) -> (ProviderBinding, BindingConversation, AgentOpenRequest) {
    setup_with_key(store, "a".repeat(64)).await
}

pub(super) async fn setup_with_key(
    store: &MongoSandboxStore,
    key_sha256: String,
) -> (ProviderBinding, BindingConversation, AgentOpenRequest) {
    let mut binding = mongo_tests::binding();
    binding.key_sha256 = key_sha256;
    binding.tool_endpoint = Some("https://gateway.example.test/mcp".into());
    store.save_binding(None, binding.clone()).await.unwrap();
    binding.work = BindingWork::ProvisionSandbox;
    binding.advance_revision(mongo_tests::NOW.into()).unwrap();
    store
        .save_binding(Some(binding.revision - 1), binding.clone())
        .await
        .unwrap();
    let parent = admission_tests::sandbox(&binding);
    let mut admission = SandboxAdmission::new(
        parent.clone(),
        SandboxAdmissionName::Exact(parent.name.clone()),
        3,
        mongo_tests::NOW.into(),
    )
    .unwrap();
    admission.binding = Some(BindingAdmissionFence::environment(&binding));
    store.admit_sandbox(admission).await.unwrap();
    let mut parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    parent.status = SandboxStatus::Active;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, mongo_tests::NOW.into());
    parent.runtime = Some(RuntimeBinding {
        runtime_id: "provider-runtime".into(),
        runtime_version: "3".into(),
        provision_generation: Some(1),
        boot_id: None,
        bound_at: mongo_tests::NOW.into(),
    });
    parent.lifecycle.runtime_generation_high_water = 1;
    parent.credential.key_generation = 1;
    parent.credential.delivered = true;
    parent.credential.current = Some(SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: format!("provided-{}", binding.key_sha256),
        provider_state_version: 1,
        issued_at: mongo_tests::NOW.into(),
        expires_at: String::new(),
        grant_fingerprint: "a".repeat(64),
    });
    parent.revision += 1;
    store.save_sandbox(1, parent).await.unwrap();
    binding.work = BindingWork::ProvisionWorkspace;
    binding.advance_revision(mongo_tests::NOW.into()).unwrap();
    store
        .save_binding(Some(binding.revision - 1), binding.clone())
        .await
        .unwrap();
    let mut workspace = admission_tests::workspace(&binding);
    store
        .admit_workspace(WorkspaceAdmission {
            binding: Some(BindingAdmissionFence::environment(&binding)),
            workspace: workspace.clone(),
            kind: WorkspaceAdmissionKind::Manual,
            event_at: mongo_tests::NOW.into(),
        })
        .await
        .unwrap();
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(mongo_tests::NOW.into());
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, mongo_tests::NOW.into());
    workspace.revision += 1;
    store.save_workspace(1, workspace.clone()).await.unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding.advance_revision(mongo_tests::NOW.into()).unwrap();
    store
        .save_binding(Some(binding.revision - 1), binding.clone())
        .await
        .unwrap();
    let conversation = BindingConversation::new(
        &binding,
        "provider_conversation".into(),
        mongo_tests::NOW.into(),
    )
    .unwrap();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    let content = binding.profile.agent.clone();
    let admission = AgentAdmission {
        binding: Some(BindingAdmissionFence::conversation(&binding, &conversation)),
        created_by: Some(layer2_core::resource_access::ResourceActor::Api {
            subject: binding.manager.clone(),
            delegation_id: None,
        }),
        owner: binding.owner.clone(),
        workspace_id: workspace.id,
        agent_id: conversation.agent_id.clone(),
        worktree_id: conversation.worktree_id.clone(),
        profile: AgentProfileSnapshot {
            id: AgentProfileId::generate(),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: conversation.creation_key().unwrap(),
        event_at: mongo_tests::NOW.into(),
    };
    store.admit_agent(admission).await.unwrap();
    let mut agent = store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    // The worker opens the conversation before publishing the Worktree ready.
    agent.work.step = AgentWorkStep::OpenConversation;
    agent.work.lease = Some(AgentWorkLease {
        holder: "provider-tool-test".into(),
        fence: agent.work.next_fence,
        expires_at: layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        ),
    });
    agent.revision += 1;
    store.save_agent(revision, agent).await.unwrap();

    let snapshot = store
        .agent_provenance_snapshot(&conversation.agent_id)
        .await
        .unwrap();
    let agent = snapshot.resource.lineage.agent.as_ref().unwrap();
    let request = AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(
            agent,
            &snapshot.resource.lineage.sandbox,
            snapshot.resource.lineage.worktree.as_ref().unwrap(),
        ),
        None,
    );
    (binding, conversation, request)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_provider_tools_retain_successful_open_and_reject_stale_authority() {
    let (client, database, store) = mongo_tests::fixture().await;
    let (binding, conversation, request) = setup(&store).await;
    let store = Arc::new(store);
    let source = MongoProviderToolSource::new(store.clone());
    let runtime = RuntimeHandle("provider-runtime".into());
    let agent = store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let fence = agent.work.next_fence;
    let snapshot = store
        .agent_provenance_snapshot(&conversation.agent_id)
        .await
        .unwrap();
    assert_provenance_fences(
        &snapshot,
        &binding,
        &conversation,
        &runtime,
        &request,
        fence,
    );

    // The real gateway binding carries an issuer-owned credential with no
    // locally retained expiry. Its optional CMA scope-revision tool is absent,
    // while the required cmaeg MCP binding below still opens and is fenced.
    let encryption = crate::store::AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([9_u8; 32]),
    )
    .unwrap();
    let access = crate::store::agent_access_tools::MongoAgentAccessToolSource::new(
        store.clone(),
        "https://control.example.test",
        &encryption,
    )
    .unwrap();
    assert!(access
        .resolve(&runtime, &request, fence)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .agent_access_tools
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );

    let mut attachment = source
        .resolve(&runtime, &request, fence)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        attachment.scope().3.endpoint,
        binding.tool_endpoint.clone().unwrap()
    );
    assert_eq!(attachment.scope().3.sandbox_id, binding.sandbox_id.as_str());
    assert!(source
        .confirm_open(&attachment, "native-conversation")
        .await
        .is_err());
    assert_eq!(
        store
            .bindings
            .tool_opens
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    attachment
        .observe(layer3_adapter::ToolProxyStatus {
            request: attachment.scope().3.clone(),
            boot_id: "b".repeat(32),
        })
        .unwrap();
    source
        .confirm_open(&attachment, "native-conversation")
        .await
        .unwrap();
    source
        .confirm_open(&attachment, "native-conversation")
        .await
        .unwrap();
    assert!(source
        .confirm_open(&attachment, "different-conversation")
        .await
        .is_err());
    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let recovered = MongoProviderToolSource::new(reopened.clone())
        .resolve(&runtime, &request, fence)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered.provenance_sha256(),
        attachment.provenance_sha256()
    );
    let document = reopened
        .bindings
        .tool_opens
        .find_one(doc! {"_id": attachment.provenance_sha256()})
        .await
        .unwrap()
        .unwrap();
    let record: OpenRecord = read_record(&document).unwrap();
    assert_eq!(record.conversation, "native-conversation");
    assert_eq!(record.boot_id, "b".repeat(32));
    assert!(source
        .resolve(&RuntimeHandle("another-runtime".into()), &request, fence)
        .await
        .is_err());
    assert!(source.resolve(&runtime, &request, fence + 1).await.is_err());
    let changed = AgentOpenRequest::new(
        request.operation_id.clone(),
        request.options.clone(),
        Some("invented-resume".into()),
    );
    assert!(source.resolve(&runtime, &changed, fence).await.is_err());
    let mut retired = conversation.clone();
    retired.state = BindingConversationState::Retiring;
    retired.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(conversation.revision), retired)
        .await
        .unwrap();
    assert!(source.resolve(&runtime, &request, fence).await.is_err());
    assert!(source
        .confirm_open(&attachment, "native-conversation")
        .await
        .is_err());
    client.database(&database).drop().await.unwrap();
}

fn assert_provenance_fences(
    snapshot: &AgentProvenanceSnapshot,
    binding: &ProviderBinding,
    conversation: &BindingConversation,
    runtime: &RuntimeHandle,
    request: &AgentOpenRequest,
    fence: u64,
) {
    let (initial, _) =
        validation::validate(snapshot, binding, conversation, runtime, request, fence).unwrap();
    for case in 0..18 {
        let mut state = snapshot.clone();
        let mut parent = binding.clone();
        let mut conversation = conversation.clone();
        match case {
            0 => conversation.generation += 1,
            1 => conversation.agent_id = layer2_core::AgentId::generate(),
            2 => parent.manager = layer2_core::SubjectId::new("another-manager"),
            3 => {
                state
                    .resource
                    .lineage
                    .agent
                    .as_mut()
                    .unwrap()
                    .profile
                    .revision += 1
            }
            4 => state.creation.as_mut().unwrap().agent_created_at = "2026-09-21T00:00:00Z".into(),
            5 => {
                state.resource.lineage.sandbox.credential.pending =
                    state.resource.lineage.sandbox.credential.current.clone()
            }
            6 => parent.tool_endpoint = Some("https://other.example/mcp".into()),
            7 => {
                state.resource.lineage.workspace.as_mut().unwrap().owner =
                    layer2_core::SubjectId::new("another-owner")
            }
            8 => {
                state.resource.lineage.worktree.as_mut().unwrap().id =
                    layer2_core::WorktreeId::generate()
            }
            9 => state.resource.lineage.sandbox.status = SandboxStatus::Paused,
            10 => {
                state
                    .resource
                    .lineage
                    .agent
                    .as_mut()
                    .unwrap()
                    .requested_stop_reason = Some(AgentStopReason::Owner)
            }
            11 => state.creation.as_mut().unwrap().created_by = None,
            12 => {
                state.resource.lineage.agent.as_mut().unwrap().work.step =
                    AgentWorkStep::MaterializeHome
            }
            13 => state.resource.lineage.agent.as_mut().unwrap().status = AgentStatus::Ready,
            14 => state.resource.lineage.worktree.as_mut().unwrap().status = WorktreeStatus::Failed,
            15 => {
                state.resource.lineage.worktree.as_mut().unwrap().status = WorktreeStatus::Removing
            }
            16 => state.resource.lineage.agent.as_mut().unwrap().work.lease = None,
            _ => {
                state
                    .resource
                    .lineage
                    .agent
                    .as_mut()
                    .unwrap()
                    .work
                    .lease
                    .as_mut()
                    .unwrap()
                    .expires_at = mongo_tests::NOW.into()
            }
        }
        assert!(
            validation::validate(&state, &parent, &conversation, runtime, request, fence).is_err(),
            "case {case}"
        );
    }
    // Reconnection before ready publication retains the same successful-open
    // authority; publishing ready changes lifecycle state, not that identity.
    let mut publishing = snapshot.clone();
    let agent = publishing.resource.lineage.agent.as_mut().unwrap();
    agent.work.step = AgentWorkStep::PublishReady;
    agent.conversation.conversation_id = Some("native-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    let (scope, _) =
        validation::validate(&publishing, binding, conversation, runtime, request, fence).unwrap();
    assert_eq!(scope, initial);
    publishing
        .resource
        .lineage
        .worktree
        .as_mut()
        .unwrap()
        .status = WorktreeStatus::Ready;
    let agent = publishing.resource.lineage.agent.as_mut().unwrap();
    agent.status = AgentStatus::Ready;
    agent.work.step = AgentWorkStep::PublishConnection;
    let (scope, _) =
        validation::validate(&publishing, binding, conversation, runtime, request, fence).unwrap();
    assert_eq!(scope, initial);
    // Key rotation changes the proof requested from the running proxy, while
    // preserving the successful-open scope and the existing ACP descriptor.
    let mut rotated = snapshot.clone();
    let mut parent = binding.clone();
    parent.key_sha256 = "c".repeat(64);
    rotated.resource.lineage.sandbox.credential.key_generation = 2;
    let authority = rotated
        .resource
        .lineage
        .sandbox
        .credential
        .current
        .as_mut()
        .unwrap();
    authority.generation = 2;
    authority.provider_key_id = format!("provided-{}", parent.key_sha256);
    let (scope, proxy) =
        validation::validate(&rotated, &parent, conversation, runtime, request, fence).unwrap();
    assert_eq!(scope.id().unwrap(), initial.id().unwrap());
    assert_eq!(proxy.credential_generation, 2);

    let mut second = initial.clone();
    second.numbered_open =
        Some(layer3_adapter::AgentProcessAction::Open.operation_id(initial.agent_id.as_str(), 2));
    let mut third = second.clone();
    third.numbered_open =
        Some(layer3_adapter::AgentProcessAction::Open.operation_id(initial.agent_id.as_str(), 3));
    assert_eq!(second.request_sha256, third.request_sha256);
    assert_ne!(second.id().unwrap(), third.id().unwrap());
    assert!(serde_json::to_value(initial)
        .unwrap()
        .get("numbered_open")
        .is_none());
}
