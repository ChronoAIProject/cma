use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::problem::ProblemCode;
use std::sync::{Arc, Mutex};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

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
            matches!(outcomes.as_slice(), [TurnOutcome::Failed { code, message }]
                if code == expected_detail && message == "provider turn could not be completed")
        );
        assert_eq!(fixture.fake.requests().len(), 1);
        fixture.fake.close().await;
    }
}

#[tokio::test]
async fn failed_starts_log_the_final_provider_call_once_inside_the_turn_span() {
    for (missing, conversation_statuses, expected_call, expected_methods) in [
        (false, vec![], "responses", vec!["POST"]),
        (
            true,
            vec![(403, "guest_denied".into())],
            "conversation",
            vec!["POST", "PUT"],
        ),
        (true, vec![], "responses", vec!["POST", "PUT", "POST"]),
    ] {
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                missing_turn_once: missing,
                conversation_statuses,
                response_refusal: Some((403, "guest_denied".into())),
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        let definition = &mut Arc::make_mut(&mut fixture.channel).definition;
        definition.provider.slug = "nyxbot".parse().unwrap();
        let request = fixture.request_with_kind(
            "refused-event",
            Some(cmaeg_core::activity::ActivityKind::Message {
                text: "private message content".into(),
                attachments: vec![],
                mentions_bot: false,
                reply_to: None,
                quoted_message: None,
                album_id: None,
            }),
        );
        let event_ref = request.event_ref.as_str().to_owned();
        let channel_id = request.channel.id.to_string();
        let channel_hash = cmaeg::telemetry::channel_hash(&channel_id);
        assert_eq!(channel_hash.len(), 16);
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let outcome = fixture
            .runner
            .launch(request)
            .with_subscriber(subscriber)
            .await;
        assert!(matches!(outcome, LaunchOutcome::Refused(problem)
            if problem.status == 502 && problem.code == ProblemCode::ProviderRejected
            && problem.detail.as_deref() == Some("provider_rejected")));
        let outcomes = fixture.terminal().await;
        assert!(
            matches!(outcomes.as_slice(), [TurnOutcome::Failed { code, message }]
            if code == "provider_rejected" && message == "provider turn could not be completed")
        );
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let warnings: Vec<_> = logs
            .lines()
            .filter(|line| line.contains("provider turn start failed"))
            .collect();
        assert_eq!(warnings.len(), 1, "{logs}");
        let line = warnings[0];
        for expected in [
            "WARN",
            "provider turn start failed",
            "cmaeg.turn",
            "event_id=refused-event",
            &format!("channel={channel_hash}"),
            "provider=\"nyxbot\"",
            &format!("provider_call=\"{expected_call}\""),
            "gateway_detail=\"provider_rejected\"",
            "gateway_status=502",
            "provider_error=\"rejected\"",
            "upstream_status=403",
            "upstream_code=\"guest_denied\"",
        ] {
            assert!(line.contains(expected), "missing {expected}: {line}");
        }
        for forbidden in [
            event_ref.as_str(),
            channel_id.as_str(),
            "runtime-test-provider-bearer",
            "private message content",
            "private provider diagnostic",
        ] {
            assert!(!line.contains(forbidden), "{line}");
        }
        assert_eq!(
            fixture
                .fake
                .requests()
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>(),
            expected_methods
        );
        fixture.fake.close().await;
    }
}
