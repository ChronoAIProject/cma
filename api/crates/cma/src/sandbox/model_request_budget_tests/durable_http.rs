use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) enum AuxiliaryKind {
    Title,
    Suggestion,
}

pub(crate) async fn auxiliary_request(
    kind: AuxiliaryKind,
    origin: &str,
    budget: Arc<dyn RequestBudget>,
    scope: RequestScope,
) -> bool {
    let context = RequestBudgetContext { budget, scope };
    match kind {
        AuxiliaryKind::Title => matches!(
            title_with_context(origin, context).await,
            Ok(AgentTitleOutcome::Generated)
        ),
        AuxiliaryKind::Suggestion => {
            let work = SuggestionWork {
                id: context.scope.request_id.clone(),
                owner: SubjectId("owner".into()),
                agent_id: context.scope.agent_id.clone(),
                scope: agent_io::AnswerContext {
                    turn_sequence: context.scope.turn_sequence,
                    answer_sequence: context.scope.turn_sequence + 1,
                },
                completed_sequence: context.scope.turn_sequence + 2,
                strategy_version: STRATEGY_VERSION.into(),
                authority: "durable-request-fixture".into(),
                status: SuggestionStatus::Running,
                context: SuggestionContext {
                    goal: "Explain reservations".into(),
                    answer: "Reserve first".into(),
                    confirmed_resources: Vec::new(),
                },
                set: None,
                created_at_ms: 0,
                started_at_ms: Some(0),
                deadline_ms: Some(1),
                observation: None,
            };
            crate::sandbox::suggestions::HttpSuggestionGenerator::new(
                origin,
                "budget-model",
                crate::auth::persistence::SecretValue::new("budget-test-bearer".into()),
                None,
            )
            .unwrap()
            .with_request_budget(context)
            .generate(&work)
            .await
            .is_ok()
        }
    }
}
