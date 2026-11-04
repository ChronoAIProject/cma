use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::problem::ProblemCode;

#[tokio::test]
async fn permanent_provider_refusals_keep_their_status_and_safe_reason_without_retrying() {
    for (status, code, expected_status, expected_detail) in [
        (
            410,
            "partition_agent_missing",
            410,
            "partition_agent_missing",
        ),
        (
            409,
            "partition_agent_unavailable",
            409,
            "partition_agent_unavailable",
        ),
        (
            409,
            "agent_command_admission_conflict",
            409,
            "agent_command_admission_conflict",
        ),
        (
            403,
            "command_authority_required",
            403,
            "command_authority_required",
        ),
        (404, "response_not_found", 404, "response_not_found"),
        (422, "model_mismatch", 422, "model_mismatch"),
        (403, "private-provider-value", 502, "provider_rejected"),
    ] {
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                response_refusal: Some((status, code.into())),
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        let LaunchOutcome::Refused(problem) = fixture
            .runner
            .launch(fixture.request("refused-event"))
            .await
        else {
            panic!("permanent refusal must not be accepted");
        };
        assert_eq!(problem.status, expected_status);
        assert_eq!(problem.code, ProblemCode::ProviderRejected);
        assert_eq!(problem.detail.as_deref(), Some(expected_detail));
        let outcomes = fixture.terminal().await;
        assert!(
            matches!(outcomes.as_slice(), [TurnOutcome::Failed { code, .. }] if code == expected_detail)
        );
        assert_eq!(fixture.fake.requests().len(), 1);
        fixture.fake.close().await;
    }
}
