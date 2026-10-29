use super::super::work_tests::message_mutation;
use super::*;
use crate::auth::{persistence::MemoryAuthStore, AuthMode, AuthState};
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::binding::*;

#[path = "binding_contention_tests.rs"]
mod contention_tests;

async fn binding_for(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> (ProviderBinding, BindingConversation, ProvidedCredential) {
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let profile = serde_json::json!({"schema_version":1,
        "metadata":{"name":"commands", "owner":{"kind":"human","subject":agent.owner.as_str()},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    let plan = map_profile(
        profile,
        &digest,
        AgentProvider::default(),
        "responses",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    let mut binding = ProviderBinding::new(
        "command-binding".into(),
        SubjectId::new("manager"),
        plan,
        key.sha256(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    binding.sandbox_id = agent.sandbox_id.clone();
    binding.workspace_id = agent.workspace_id.clone();
    store.save_binding(None, binding.clone()).await.unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    let mut conversation = BindingConversation::new(
        &binding,
        "conversation-command".into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    conversation.agent_id = agent.id.clone();
    conversation.worktree_id = agent.worktree_id.clone();
    store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    store
        .save_binding_conversation(binding.revision, Some(1), conversation.clone())
        .await
        .unwrap();
    (binding, conversation, key)
}
fn auth(store: Arc<MongoSandboxStore>) -> Arc<AuthState> {
    let unavailable = Arc::new(MemoryAuthStore::default());
    unavailable.set_unavailable(true);
    AuthState::with_store(
        AuthMode::Trusted {
            subject: "unrelated-owner".into(),
        },
        None,
        unavailable,
    )
    .with_binding_store(store)
}
fn context(principal: &BindingPrincipal) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Binding,
        caller: principal.caller(),
        credential: CredentialContext {
            issuer: "binding:local".into(),
            subject: principal.owner().clone(),
            credential_id: principal.key_sha256().into(),
        },
        actor: ResourceActor::Api {
            subject: principal.owner().clone(),
            delegation_id: None,
        },
        memberships: vec![],
    }
}
async fn reservation(
    store: &Arc<MongoSandboxStore>,
    agent: &Agent,
    parent: &Sandbox,
    binding: &ProviderBinding,
) -> CommandReservation {
    let bindings = BindingAccessService::new(store.clone());
    let principal = bindings
        .authenticate(&binding.key_sha256)
        .await
        .unwrap()
        .unwrap();
    let mut request = live_request(agent, parent, "binding-event");
    request.binding = Some(
        bindings
            .resolve(&principal, agent.id.as_str())
            .await
            .unwrap(),
    );
    request.envelope.actor = context(&principal).actor;
    request.envelope.source.kind = AgentSourceKind::Api;
    request.envelope.authorization = principal.command_authority();
    request
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_key_rotation_preserves_replay_and_deferred_native_dispatch() {
    let (client, name) = database("binding_command_rotation").await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let (binding, _, _) = binding_for(&store, &agent).await;
    let old = reservation(&store, &agent, &parent, &binding).await;
    let original = record(store.reserve_command(old.clone()).await.unwrap());
    let next = binding
        .rotate_credential("b".repeat(64), layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(binding.revision), next.clone())
        .await
        .unwrap();
    assert_eq!(
        store.reserve_command(old).await,
        Err(CommandLedgerError::Conflict)
    );
    let replacement = reservation(&store, &agent, &parent, &next).await;
    assert_eq!(
        store.reserve_command(replacement).await.unwrap(),
        CommandReservationResult::Existing(Box::new(original))
    );
    let command = store
        .claim_next_command(claim_request(
            &agent,
            "binding-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let auth = auth(store.clone());
    let access = ResourceAccessService::new(store.clone(), auth.clone())
        .with_bindings(BindingAccessService::new(store.clone()));
    let proof = auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .unwrap();
    assert_eq!(
        proof.caller().authorization,
        Authorization::new([Capability::UseBindingConversations])
    );
    execution
        .operations
        .execute_resource_claim(&proof, &command)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let retained = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.receipt, command.receipt);
    assert!(retained.work.dispatch.is_some());
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        current.conversation.conversation_id,
        agent.conversation.conversation_id
    );
    assert_eq!(current.worktree_id, agent.worktree_id);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_retirement_and_rotation_fence_admission_and_dispatch_transactions() {
    for change in ["conversation", "binding", "key", "missing-proof"] {
        let (client, name) = database("binding_command_fence").await;
        let (store, agent, parent, _, _, sends) = composed(&client, &name).await;
        let (mut binding, mut conversation, _) = binding_for(&store, &agent).await;
        let request = reservation(&store, &agent, &parent, &binding).await;
        store.reserve_command(request.clone()).await.unwrap();
        let command = store
            .claim_next_command(claim_request(
                &agent,
                "binding-worker",
                &layer2_core::now_rfc3339(),
            ))
            .await
            .unwrap()
            .unwrap();
        let auth = auth(store.clone());
        let access = ResourceAccessService::new(store.clone(), auth.clone())
            .with_bindings(BindingAccessService::new(store.clone()));
        let proof = auth
            .authorize_resource_command(&access, &command.receipt.envelope)
            .await
            .unwrap();
        let mut mutation =
            message_mutation(&agent, &parent, &command.work.lease.as_ref().unwrap().claim);
        mutation.execution_fence = Some(proof.fence());
        match change {
            "conversation" => {
                conversation.state = BindingConversationState::Retiring;
                conversation.revision += 1;
                store
                    .save_binding_conversation(binding.revision, Some(2), conversation)
                    .await
                    .unwrap();
            }
            "binding" => {
                let revision = binding.revision;
                binding.state = BindingState::Retired;
                binding.work = BindingWork::RetireCurrentConversations;
                binding
                    .advance_revision(layer2_core::now_rfc3339())
                    .unwrap();
                store.save_binding(Some(revision), binding).await.unwrap();
            }
            "key" => {
                let next = binding
                    .rotate_credential("b".repeat(64), layer2_core::now_rfc3339())
                    .unwrap();
                store
                    .save_binding(Some(binding.revision), next)
                    .await
                    .unwrap();
            }
            _ => mutation.execution_fence = None,
        }
        if change != "missing-proof" {
            assert_eq!(
                store.reserve_command(request).await,
                Err(CommandLedgerError::Conflict)
            );
        }
        assert!(
            store.commit_agent_conversation(mutation).await.is_err(),
            "{change}"
        );
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command)
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert_eq!(sends.load(Ordering::SeqCst), 0);
        client.database(&name).drop().await.unwrap();
    }
}

#[path = "binding_revival_tests.rs"]
mod revival;

#[path = "binding_event_tests.rs"]
mod event_context_tests;

#[path = "media_continuation_tests.rs"]
mod media_continuation_tests;
