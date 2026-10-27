use super::*;
#[test]
fn history_consent_requires_its_independent_gate_and_can_be_retained_or_removed() {
    let mut next = definition(
        json!([{"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"},"conversation_history":true}]),
    );
    next.secrets.discord_bot_token = Some(Secret::new("history-fixture"));
    let flags = WriteFlags {
        native_discord: true,
        discord_thread_history: true,
        discord_thread_lifecycle: true,
        ..Default::default()
    };
    assert_eq!(
        check(None, &next, flags).unwrap_err(),
        ValidationError::new(
            "/sources/0/conversation_history",
            "discord_conversation_history_disabled"
        )
    );
    check(
        None,
        &next,
        WriteFlags {
            discord_conversation_history: true,
            ..flags
        },
    )
    .unwrap();
    check(Some(&next), &next, flags).unwrap();
    let mut removed = next.clone();
    let cmaeg_core::channel::Source::Discord {
        conversation_history,
        ..
    } = &mut removed.sources[0]
    else {
        panic!()
    };
    *conversation_history = false;
    check(Some(&next), &removed, flags).unwrap();
}
