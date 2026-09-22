use super::tests::{key, reserved};
use super::*;
use crate::auth::{persistence::MemoryAuthStore, AuthMode};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::binding::{
    BindingConversation, BindingConversationState, BindingState, BindingWork,
};
use layer2_core::sandbox::InMemorySandboxStore;

#[tokio::test]
async fn binding_deferred_authority_stays_local_and_survives_key_rotation() {
    let auth_store = Arc::new(MemoryAuthStore::default());
    auth_store.set_unavailable(true);
    let store = Arc::new(InMemorySandboxStore::default());
    let auth = AuthState::with_store(
        AuthMode::Trusted {
            subject: "unrelated-owner".into(),
        },
        None,
        auth_store,
    )
    .with_binding_store(store.clone());
    let original = key();
    let mut binding = reserved(&original);
    store.save_binding(None, binding.clone()).await.unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    let mut conversation = BindingConversation::new(
        &binding,
        "conversation-local".into(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
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
    let request = auth
        .binding_request(original.expose().into())
        .await
        .unwrap()
        .unwrap();
    let context = auth.resource_request_context(&request);
    let owner = AgentOwner::Person {
        subject: binding.owner.clone(),
    };
    let authority = auth
        .command_admission_authority(&context, original.expose(), &owner)
        .await
        .unwrap();
    assert_eq!(
        authority.authority_id,
        format!("binding:{}", binding.binding_id)
    );
    assert_eq!(authority.grant_revision, Some(binding.generation));
    let command = AgentCommand::message("binding message");
    let envelope = AgentCommandEnvelope {
        schema_version: AGENT_IO_SCHEMA_VERSION,
        command_id: layer2_core::ids::AgentCommandId::generate(),
        agent_id: conversation.agent_id,
        owner: owner.clone(),
        actor: context.actor.clone(),
        source: AgentCommandSource {
            kind: AgentSourceKind::Api,
            reference: "binding-test".into(),
        },
        correlation_id: "binding-test".into(),
        request_fingerprint: command.fingerprint().unwrap(),
        authorization: authority,
        command,
    };
    assert_eq!(
        auth.authorize_command(&envelope).await.unwrap(),
        context.caller
    );
    let replacement = key();
    let rotated = binding
        .rotate_credential(replacement.sha256(), layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(binding.revision), rotated.clone())
        .await
        .unwrap();
    assert_eq!(
        auth.authorize_command(&envelope).await.unwrap(),
        context.caller
    );
    assert!(auth
        .command_admission_authority(&context, original.expose(), &owner)
        .await
        .is_err());
    for case in 0..4 {
        let mut forged = envelope.clone();
        match case {
            0 => {
                forged.actor = AgentActor::Human {
                    subject: binding.owner.clone(),
                }
            }
            1 => forged.source.kind = AgentSourceKind::UserInterface,
            2 => forged.authorization.grant_revision = Some(binding.generation + 1),
            _ => forged.agent_id = layer2_core::AgentId::generate(),
        }
        assert!(auth.authorize_command(&forged).await.is_err());
    }
    let mut retired = rotated.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(rotated.revision), retired)
        .await
        .unwrap();
    assert!(auth.authorize_command(&envelope).await.is_err());
}
