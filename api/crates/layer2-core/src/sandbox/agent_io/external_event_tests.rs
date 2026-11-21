use super::*;
use serde_json::json;

fn context() -> ExternalEventContext {
    ExternalEventContext {
        provider_conversation_id: None,
        source_id: "source".into(),
        conversation_id: "conversation".into(),
        event_id: "event".into(),
        reference: "sealed-reference".into(),
        payload: json!({"body":"x".repeat(300_000), "nested":{"keep":true}}),
    }
}
#[test]
fn full_event_is_fingerprinted_but_only_its_reference_reaches_runtime_text() {
    let mut command = AgentCommand::message("hello");
    let AgentCommand::Message { input, .. } = &mut command else {
        unreachable!()
    };
    input
        .content
        .push(AgentContent::ExternalEvent { context: context() });
    assert_eq!(
        input.clone().into_text().unwrap(),
        "hello\n\nevent_ref: sealed-reference"
    );
    let first = command.fingerprint().unwrap();
    let mut next = command.clone();
    let AgentCommand::Message { input, .. } = &mut next else {
        unreachable!()
    };
    let AgentContent::ExternalEvent { context } = &mut input.content[1] else {
        unreachable!()
    };
    context.payload["nested"]["keep"] = false.into();
    assert_ne!(first, next.fingerprint().unwrap());
    let wire = serde_json::to_value(&command).unwrap();
    assert_eq!(
        serde_json::from_value::<AgentCommand>(wire).unwrap(),
        command
    );
}
#[test]
fn external_context_limits_preserve_the_existing_runtime_input_bound() {
    let external = AgentContent::ExternalEvent { context: context() };
    let mut input = AgentInput::text("hello");
    input.content.extend([external.clone(), external.clone()]);
    assert!(input.canonical().is_err());
    let mut input = AgentInput::text("x".repeat(crate::sandbox::MAX_AGENT_MESSAGE_BYTES));
    input.content.push(external.clone());
    assert!(input.canonical().is_err());
    let mut input = AgentInput::text("hello");
    input.content.push(external);
    assert!(AgentCommand::Steer { input }.canonical().is_err());
    let mut context = context();
    context.payload = json!({"body":"x".repeat(MAX_EXTERNAL_EVENT_BYTES)});
    assert!(context.validate().is_err());
}
