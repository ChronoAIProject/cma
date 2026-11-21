use super::super::conversation_fold::{fold_update, FoldedConversationUpdate};
use super::super::*;
use super::fixtures::*;
use layer3_adapter::{ConversationMetadata, ConversationUpdate, ConversationUsage};

fn apply(agent: &mut Agent, update: ConversationUpdate) -> Vec<AgentLogKind> {
    match fold_update(
        agent,
        &AgentMessageAssembly::default(),
        update,
        LATER,
        false,
    )
    .unwrap()
    {
        FoldedConversationUpdate::Changed(change) => {
            *agent = change.agent;
            change.logs
        }
        FoldedConversationUpdate::Quiet(_) => vec![],
    }
}
fn usage(
    turn: &str,
    input: u64,
    output: u64,
    last: (u64, u64),
    context: u64,
) -> ConversationUpdate {
    ConversationUpdate::Usage(ConversationUsage {
        turn_id: turn.into(),
        input_tokens: input,
        output_tokens: output,
        last_input_tokens: last.0,
        last_output_tokens: last.1,
        context_used: context,
        context_window: Some(10000),
        counters_reset: false,
    })
}
#[test]
fn runtime_metadata_persists_only_canonical_observations_and_deduplicates() {
    let mut agent = test_agent(AgentStatus::Ready, None);
    let workspace = agent.workspace_id.clone();
    let worktree = agent.worktree_id.clone();
    let observation = ConversationUpdate::Metadata(ConversationMetadata {
        working_directory: Some("/home/cma".into()),
        description: Some("Investigate checkout".into()),
        ..Default::default()
    });
    assert!(matches!(
        apply(&mut agent, observation.clone()).as_slice(),
        [AgentLogKind::RuntimeMetadataChanged { .. }]
    ));
    assert!(apply(&mut agent, observation).is_empty());
    apply(
        &mut agent,
        ConversationUpdate::Metadata(ConversationMetadata {
            working_directory: Some("relative/not-canonical".into()),
            description: None,
            ..Default::default()
        }),
    );
    assert_eq!(
        agent
            .conversation
            .observations
            .metadata
            .working_directory
            .as_deref(),
        Some("/home/cma")
    );
    assert_eq!(agent.workspace_id, workspace);
    assert_eq!(agent.worktree_id, worktree);
    let restored: Agent = serde_json::from_value(serde_json::to_value(&agent).unwrap()).unwrap();
    assert_eq!(
        restored.conversation.observations,
        agent.conversation.observations
    );
}
#[test]
fn usage_accounts_every_call_without_context_compaction_or_resume_resets() {
    let mut agent = test_agent(AgentStatus::Working, None);
    apply(&mut agent, usage("turn-one", 100, 10, (100, 10), 110));
    apply(&mut agent, usage("turn-one", 180, 35, (80, 25), 105));
    assert!(apply(&mut agent, usage("turn-one", 180, 35, (80, 25), 105)).is_empty());
    // Compaction can reduce context without reducing lifetime or turn usage.
    apply(&mut agent, usage("turn-one", 180, 35, (0, 0), 20));
    agent.conversation.runtime_generation = Some(4);
    assert!(apply(&mut agent, usage("turn-one", 180, 35, (0, 0), 20)).is_empty());
    let count = agent.conversation.observations.usage.as_ref().unwrap();
    assert_eq!(
        (count.input_tokens, count.output_tokens, count.context_used),
        (180, 35, 20)
    );
    assert_eq!(
        (
            count.current_turn.unwrap().input_tokens,
            count.current_turn.unwrap().output_tokens
        ),
        (180, 35)
    );
    agent.conversation.active_turn = Some(100);
    apply(&mut agent, usage("turn-two", 220, 40, (40, 5), 45));
    let count = agent.conversation.observations.usage.as_ref().unwrap();
    assert_eq!((count.input_tokens, count.output_tokens), (220, 40));
    assert_eq!(
        count.current_turn,
        Some(AgentTurnUsage {
            turn_sequence: 100,
            input_tokens: 40,
            output_tokens: 5
        })
    );
}
#[test]
fn explicit_counter_reset_is_not_consumption_and_regressing_replay_is_ignored() {
    let mut agent = test_agent(AgentStatus::Working, None);
    apply(&mut agent, usage("turn-one", 100, 20, (100, 20), 120));
    assert!(apply(&mut agent, usage("turn-one", 80, 10, (80, 10), 90)).is_empty());
    let mut reset = match usage("turn-one", 0, 0, (0, 0), 10000) {
        ConversationUpdate::Usage(u) => u,
        _ => unreachable!(),
    };
    reset.counters_reset = true;
    apply(&mut agent, ConversationUpdate::Usage(reset));
    apply(&mut agent, usage("turn-one", 30, 4, (30, 4), 34));
    let count = agent.conversation.observations.usage.as_ref().unwrap();
    assert_eq!((count.input_tokens, count.output_tokens), (130, 24));
    assert_eq!(
        (
            count.current_turn.unwrap().input_tokens,
            count.current_turn.unwrap().output_tokens
        ),
        (130, 24)
    );
}
#[test]
fn first_historical_snapshot_does_not_attribute_entire_lifetime_to_current_turn() {
    let mut agent = test_agent(AgentStatus::Working, None);
    apply(&mut agent, usage("turn-current", 500, 100, (30, 5), 35));
    let count = agent.conversation.observations.usage.as_ref().unwrap();
    assert_eq!((count.input_tokens, count.output_tokens), (500, 100));
    assert_eq!(
        (
            count.current_turn.unwrap().input_tokens,
            count.current_turn.unwrap().output_tokens
        ),
        (30, 5)
    );
    let logs = apply(
        &mut agent,
        ConversationUpdate::TurnEnded(
            layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn)
                .usage(layer3_adapter::TurnUsage::new(35, 30, 5)),
        ),
    );
    assert!(!logs
        .iter()
        .any(|log| matches!(log, AgentLogKind::Usage { .. })));
}

#[test]
fn title_provenance_binds_setup_input_and_never_downgrades_explicit_titles() {
    use layer3_adapter::{
        ConversationDescriptionInput as Input, ConversationDescriptionSource as Source,
    };
    let mut agent = test_agent(AgentStatus::Working, None);
    let mut message = FirstMessageRecord::new("lifecycle", "Setup exact\npayload", LATER).unwrap();
    message.purpose = Some(FirstMessagePurpose::Setup);
    message.turn_sequence = agent.conversation.active_turn;
    let text = message.text.clone();
    agent.conversation.first_message = Some(message);
    let preview = |input| {
        ConversationUpdate::Metadata(ConversationMetadata {
            description: Some("Setup exact payload".into()),
            description_source: Some(Source::PromptPreview),
            description_input: input,
            ..Default::default()
        })
    };
    apply(&mut agent, preview(Some(Input::Current(text.clone()))));
    assert!(
        agent
            .conversation
            .observations
            .metadata
            .description_is_setup_preview
    );
    agent.conversation.active_turn = None;
    apply(&mut agent, preview(None));
    assert!(
        agent
            .conversation
            .observations
            .metadata
            .description_is_setup_preview
    );
    apply(&mut agent, preview(Some(Input::First(text.clone()))));
    assert!(
        agent
            .conversation
            .observations
            .metadata
            .description_is_setup_preview
    );
    // The same payload on a later actual task is not the recorded setup turn.
    agent.conversation.active_turn = Some(999);
    apply(&mut agent, preview(Some(Input::Current(text.clone()))));
    assert!(
        !agent
            .conversation
            .observations
            .metadata
            .description_is_setup_preview
    );
    apply(
        &mut agent,
        ConversationUpdate::Metadata(ConversationMetadata {
            description: Some("Setup exact payload".into()),
            description_source: Some(Source::ProviderTitle),
            ..Default::default()
        }),
    );
    assert!(
        !agent
            .conversation
            .observations
            .metadata
            .description_is_setup_preview
    );
    apply(&mut agent, preview(Some(Input::First(text))));
    apply(
        &mut agent,
        ConversationUpdate::Metadata(ConversationMetadata {
            description: Some("old replay".into()),
            working_directory: Some("/home/cma/new".into()),
            ..Default::default()
        }),
    );
    let metadata = &agent.conversation.observations.metadata;
    assert_eq!(metadata.description.as_deref(), Some("Setup exact payload"));
    assert_eq!(metadata.description_source, Some(Source::ProviderTitle));
    assert!(!metadata.description_is_setup_preview);
    assert_eq!(metadata.working_directory.as_deref(), Some("/home/cma/new"));
}

#[test]
fn missing_provenance_stays_unknown_and_payload_identity_is_unchanged() {
    let message = FirstMessageRecord::new("legacy", "Exact payload", LATER).unwrap();
    let mut persisted = serde_json::to_value(&message).unwrap();
    persisted.as_object_mut().unwrap().remove("purpose");
    let restored: FirstMessageRecord = serde_json::from_value(persisted).unwrap();
    assert_eq!(restored.purpose, None);
    assert_eq!(restored.payload_sha256, message.payload_sha256);
    let metadata: AgentRuntimeMetadata =
        serde_json::from_value(serde_json::json!({"description":"legacy title"})).unwrap();
    assert_eq!(metadata.description_source, None);
    assert!(!metadata.description_is_setup_preview);
}
