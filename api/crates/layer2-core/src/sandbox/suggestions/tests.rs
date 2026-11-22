use super::*;

#[test]
fn a_running_read_stays_nonterminal_until_the_timeout_is_persisted() {
    let mut work = SuggestionWork {
        id: "suggestion-clock".into(),
        owner: SubjectId("owner".into()),
        agent_id: AgentId("agt_clock".into()),
        scope: AnswerContext {
            turn_sequence: 3,
            answer_sequence: 5,
        },
        completed_sequence: 6,
        strategy_version: STRATEGY_VERSION.into(),
        authority: "synthetic-authority".into(),
        status: SuggestionStatus::Running,
        context: SuggestionContext {
            goal: "Compare local storage options.".into(),
            answer: "SQLite suits a local-only app.".into(),
            confirmed_resources: Vec::new(),
        },
        set: None,
        created_at_ms: 1,
        started_at_ms: Some(1),
        deadline_ms: Some(70_001),
        observation: None,
    };
    let before = work.clone();
    let reading = work.reading(true);
    assert_eq!(reading.status, SuggestionStatus::Running);
    assert!(reading.set.is_none());
    assert!(reading.observation.is_none());
    assert_eq!(work, before);
    assert_eq!(work.reading(false).status, SuggestionStatus::Expired);

    assert!(!work.claim(true, 70_501));
    let reading = work.reading(true);
    assert_eq!(reading.status, SuggestionStatus::Failed);
    assert_eq!(
        reading.observation.unwrap().failure,
        Some(SuggestionFailure::Interrupted)
    );
}
