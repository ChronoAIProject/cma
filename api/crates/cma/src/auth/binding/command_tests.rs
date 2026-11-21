use super::tests::{key, reserved_with_owner};
use super::*;
use crate::auth::{persistence::MemoryAuthStore, AuthMode};
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::binding::{
    BindingConversation, BindingConversationState, BindingState, BindingWork,
};
use layer2_core::sandbox::InMemorySandboxStore;

#[tokio::test]
async fn binding_deferred_authority_stays_local_and_survives_key_rotation() {
    deferred_authority("human").await;
}

#[tokio::test]
async fn organization_binding_admits_turns_and_retains_narrow_deferred_authority() {
    deferred_authority("organization").await;
}

async fn deferred_authority(owner_kind: &str) {
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
    let mut binding = reserved_with_owner(&original, owner_kind);
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
    let owner = if owner_kind == "organization" {
        AgentOwner::Organization {
            subject: binding.owner.clone(),
        }
    } else {
        AgentOwner::Person {
            subject: binding.owner.clone(),
        }
    };
    let wrong_kind = if owner_kind == "organization" {
        AgentOwner::Person {
            subject: binding.owner.clone(),
        }
    } else {
        AgentOwner::Organization {
            subject: binding.owner.clone(),
        }
    };
    let wrong_subject = if owner_kind == "organization" {
        AgentOwner::Organization {
            subject: layer2_core::SubjectId::new("other-owner"),
        }
    } else {
        AgentOwner::Person {
            subject: layer2_core::SubjectId::new("other-owner"),
        }
    };
    for forged in [&wrong_kind, &wrong_subject] {
        assert!(auth
            .command_admission_authority(&context, original.expose(), forged)
            .await
            .is_err());
    }
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
        continuation: None,
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
    for case in 0..6 {
        let mut forged = envelope.clone();
        match case {
            0 => {
                forged.actor = AgentActor::Human {
                    subject: binding.owner.clone(),
                }
            }
            1 => forged.source.kind = AgentSourceKind::UserInterface,
            2 => forged.authorization.grant_revision = Some(binding.generation + 1),
            3 => forged.agent_id = layer2_core::AgentId::generate(),
            4 => forged.owner = wrong_kind.clone(),
            _ => forged.owner = wrong_subject.clone(),
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

#[test]
fn media_continuation_routes_are_scoped_to_the_exact_bound_consumer() {
    use axum::http::Method;
    let base = "/api/v1/cmaeg/bindings/binding-a/conversations/transport/events/original/media/0";
    for (method, suffix) in [
        (Method::POST, "attachment"),
        (Method::GET, "attachment"),
        (Method::POST, "supplement"),
    ] {
        let path = format!("{base}/{suffix}");
        assert!(super::permits_request(&method, &path, "binding-a"));
        assert!(!super::permits_request(&method, &path, "binding-b"));
        assert!(!super::permits_request(&Method::DELETE, &path, "binding-a"));
    }
    assert!(!super::permits_request(
        &Method::GET,
        &format!("{base}/supplement"),
        "binding-a"
    ));
    assert!(!super::permits_request(
        &Method::POST,
        "/api/v1/agents/agt_foreign/attachments",
        "binding-a"
    ));
}
