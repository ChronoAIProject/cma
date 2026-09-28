use super::{
    command_tests::{request, LocalAuthority},
    resource_access::{context, ready_conversation},
    *,
};
use crate::{resource_access::ResourceAccessService, sandbox::agent_io::*};
use serde_json::json;

fn attach(request: &mut CommandReservation) {
    let AgentCommand::Message { input, .. } = &mut request.envelope.command else {
        unreachable!()
    };
    input.content.push(AgentContent::ExternalEvent {
        context: ExternalEventContext {
            provider_conversation_id: None,
            source_id: "source".into(),
            conversation_id: "scoped-conversation".into(),
            event_id: "event".into(),
            reference: "sealed-reference".into(),
            payload: json!({"body":"x".repeat(300_000), "nested":{"keep":true}}),
        },
    });
    request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
}
#[tokio::test]
async fn external_event_is_atomic_immutable_and_confined_to_current_binding() {
    let f = fixture();
    let binding = f.ready().await;
    let conversation = ready_conversation(&f, &binding).await;
    let bindings = BindingAccessService::new(f.store.clone());
    let authority = Arc::new(LocalAuthority(bindings.clone()));
    let access = Arc::new(
        ResourceAccessService::new(f.store.clone(), authority.clone())
            .with_bindings(bindings.clone()),
    );
    let commands = AgentCommandService::new(f.store.clone(), access.clone(), authority);
    let mut first = request(&f, &bindings, &access, "event-one").await;
    attach(&mut first);
    let mut other = first.clone();
    other.key_sha256 = command_key_digest("event-other-key").unwrap();
    other.envelope.command_id = crate::ids::AgentCommandId::generate();
    other.response_id = Some(crate::ids::AgentResponseId::generate());
    let (a, b) = tokio::join!(
        f.store.reserve_command(first.clone()),
        f.store.reserve_command(other)
    );
    assert!(a.is_ok());
    assert_eq!(b, Err(CommandLedgerError::IdempotencyConflict));
    assert!(matches!(
        f.store.reserve_command(first.clone()).await,
        Ok(CommandReservationResult::Existing(_))
    ));
    let principal = bindings
        .authenticate(&f.key.sha256())
        .await
        .unwrap()
        .unwrap();
    let expected = first.envelope.command.external_event().unwrap();
    let read = commands
        .external_event_context(
            &context(&principal),
            None,
            &conversation.agent_id,
            &binding.binding_id,
            "scoped-conversation",
            "event",
        )
        .await
        .unwrap();
    assert_eq!(&read, expected);
    let mut changed = first.clone();
    let AgentCommand::Message { input, .. } = &mut changed.envelope.command else {
        unreachable!()
    };
    let AgentContent::ExternalEvent { context } = &mut input.content[1] else {
        unreachable!()
    };
    context.payload["nested"]["keep"] = false.into();
    changed.envelope.request_fingerprint = changed.envelope.command.fingerprint().unwrap();
    assert_eq!(
        f.store.reserve_command(changed).await,
        Err(CommandLedgerError::IdempotencyConflict)
    );
    assert!(commands
        .external_event_context(
            &super::resource_access::context(&principal),
            None,
            &conversation.agent_id,
            "foreign-binding",
            "scoped-conversation",
            "event"
        )
        .await
        .is_err());
    let mut no_proof = first.clone();
    no_proof.binding = None;
    assert_eq!(
        f.store.reserve_command(no_proof).await,
        Err(CommandLedgerError::Invalid)
    );
    f.service
        .retire(&f.caller, &binding.binding_id)
        .await
        .unwrap();
    assert!(commands
        .external_event_context(
            &super::resource_access::context(&principal),
            None,
            &conversation.agent_id,
            &binding.binding_id,
            "scoped-conversation",
            "event"
        )
        .await
        .is_err());
    let retained = f
        .store
        .get_external_event_command(&first.envelope.external_event_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.receipt.envelope.command.external_event(),
        Some(expected)
    );
}
