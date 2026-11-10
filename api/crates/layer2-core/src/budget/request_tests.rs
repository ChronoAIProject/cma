use super::*;

fn policy() -> BudgetPolicy {
    BudgetPolicy {
        owner_day_limit: u64::from(u32::MAX),
        turn_limit: u64::from(u16::MAX),
        input_token_units: 2,
        output_token_units: 3,
        max_unobserved_units: 1,
        usage_timeout_seconds: 30,
    }
}

#[test]
fn request_quote_accounts_for_the_full_input_envelope_and_output_limit() {
    let model = RequestModelPolicy {
        model: "approved".into(),
        input_overhead_tokens: 17,
    };
    let request = ModelRequest {
        model: "approved".into(),
        input_bytes: 43,
        max_output_tokens: 19,
        fingerprint: "a".repeat(64),
    };
    assert_eq!(model.quote(&policy(), &request), Ok((43 + 17) * 2 + 19 * 3));
    let mut wrong_model = request.clone();
    wrong_model.model = "unknown".into();
    assert_eq!(
        model.quote(&policy(), &wrong_model),
        Err(BudgetError::Unavailable)
    );
    let mut unbounded = request;
    unbounded.max_output_tokens = u64::MAX;
    assert_eq!(
        model.quote(&policy(), &unbounded),
        Err(BudgetError::Unavailable)
    );
}

#[test]
fn request_quotes_and_owner_holds_refuse_overflow_without_mutating_accounting() {
    let mut policy = policy();
    policy.input_token_units = (1_u64 << 53) - 1;
    let model = RequestModelPolicy {
        model: "approved".into(),
        input_overhead_tokens: 1,
    };
    let request = ModelRequest {
        model: "approved".into(),
        input_bytes: 32 * 1024,
        max_output_tokens: 1,
        fingerprint: "a".repeat(64),
    };
    assert_eq!(
        model.quote(&policy, &request),
        Err(BudgetError::Unavailable)
    );
    let mut owner = OwnerBudget::default();
    owner
        .reserve_liability(&policy, policy.owner_day_limit, 1)
        .unwrap();
    let before = owner.clone();
    assert_eq!(
        owner.reserve_liability(&policy, 1, 1),
        Err(BudgetError::OwnerExhausted)
    );
    assert_eq!(owner, before);
}

#[test]
fn model_bound_configuration_has_no_implicit_allowed_models() {
    let mut settings = BudgetSettings::default();
    assert!(settings.request_models.is_empty());
    let model = RequestModelPolicy {
        model: "approved".into(),
        input_overhead_tokens: 1,
    };
    settings.request_models = vec![model.clone(), model];
    assert_eq!(settings.validate(), Err(BudgetError::InvalidPolicy));
}
