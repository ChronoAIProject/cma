use super::acp::Outcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CreditRefusalDiagnostic {
    pub(super) http_status: u16,
    pub(super) provider_error_code: u32,
    pub(super) failure_class: String,
    pub(super) agent_correlation_digest: String,
    pub(super) prompt_id_hash: String,
}

pub(super) fn credit_refusal_diagnostic(
    outcome: &Outcome,
    agent_correlation_digest: &str,
    prompt_id: Option<&crate::PromptIdentity>,
) -> Option<CreditRefusalDiagnostic> {
    let Outcome::Failed {
        capability: Some(capability),
        ..
    } = outcome
    else {
        return None;
    };
    if capability != "insufficient_credits" {
        return None;
    }
    Some(CreditRefusalDiagnostic {
        http_status: 402,
        provider_error_code: 11300,
        failure_class: capability.clone(),
        agent_correlation_digest: agent_correlation_digest.to_owned(),
        prompt_id_hash: prompt_id
            .map(crate::PromptIdentity::as_str)
            .unwrap_or("none")
            .to_owned(),
    })
}

pub(super) fn trace_credit_refusal(
    outcome: &Outcome,
    agent_correlation_digest: &str,
    prompt_id: Option<&crate::PromptIdentity>,
) {
    let Some(diagnostic) = credit_refusal_diagnostic(outcome, agent_correlation_digest, prompt_id)
    else {
        return;
    };
    tracing::warn!(
        target: "cma::model_gateway",
        event = "model_gateway_credit_refusal",
        source = "managed_model_gateway",
        http_status = diagnostic.http_status,
        provider_error_code = diagnostic.provider_error_code,
        failure_class = %diagnostic.failure_class,
        agent_correlation_digest = %diagnostic.agent_correlation_digest,
        prompt_id_hash = %diagnostic.prompt_id_hash,
        "managed model gateway refused a model request for insufficient credits"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_refusal_diagnostic_contains_only_safe_correlation_fields() {
        let prompt = crate::PromptIdentity::new("conversation", 2, 7);
        let outcome = Outcome::Failed {
            capability: Some("insufficient_credits".into()),
            message: "private upstream body".into(),
        };
        let diagnostic =
            credit_refusal_diagnostic(&outcome, "agent-digest", Some(&prompt)).unwrap();
        assert_eq!(diagnostic.http_status, 402);
        assert_eq!(diagnostic.provider_error_code, 11300);
        assert_eq!(diagnostic.failure_class, "insufficient_credits");
        assert_eq!(diagnostic.agent_correlation_digest, "agent-digest");
        assert_eq!(diagnostic.prompt_id_hash, prompt.as_str());
        assert!(!format!("{diagnostic:?}").contains("private upstream body"));
        assert!(credit_refusal_diagnostic(
            &Outcome::Ended(crate::PromptResponse::new(crate::TurnStopReason::EndTurn)),
            "agent-digest",
            Some(&prompt),
        )
        .is_none());
    }
}
