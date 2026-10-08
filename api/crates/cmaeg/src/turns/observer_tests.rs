use super::*;
use crate::ingress::launch::LaunchRefusal;
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    ids::ConsumerId,
    problem::{Problem, ProblemCode},
};

#[test]
fn summaries_exclude_payloads_and_mask_identities_before_observation() {
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    activity.actor.id = "private-actor-1234".into();
    activity.conversation.id = "private-chat-5678".into();
    activity.kind = ActivityKind::Command {
        name: "private-command".into(),
        args: "private-arguments".into(),
    };
    let summary = ActivitySummary::of(&activity);
    assert_eq!(summary.actor_masked, "…1234");
    assert_eq!(summary.chat_masked, "…5678");
    assert_eq!(summary.event_key.len(), 16);
    assert_eq!(summary.kind, Kind::Command);
    let shown = format!("{summary:?}");
    for raw in [
        "private-actor",
        "private-chat",
        "private-command",
        "private-arguments",
        activity.event_id.as_str(),
    ] {
        assert!(!shown.contains(raw), "raw content entered a summary");
    }
    for value in ["", "1234", "éééé"] {
        assert_eq!(summary::masked_id(value), "…");
    }
    assert_eq!(summary::masked_id("ou_ünïcødé"), "…cødé");
}

#[test]
fn launch_summary_retains_only_classification_and_bounded_provider_code() {
    let mut refusal = LaunchRefusal::new(
        Problem::new(ProblemCode::ProviderUnavailable, 503, "private detail"),
        "provider_rejected",
    );
    for (input, expected) in [
        ("access_denied", Some("access_denied")),
        ("private provider detail", None),
    ] {
        refusal.provider_code = Some(input.into());
        let summary = OutcomeSummary::of(
            "private-event",
            ConsumerId::default_consumer(),
            &LaunchOutcome::Refused(refusal.clone()),
            Duration::MAX,
        );
        assert_eq!(summary.class, LaunchClass::Refused);
        assert_eq!(summary.provider_code.as_deref(), expected);
        assert_eq!(summary.accepted_ms, None);
        assert!(!format!("{summary:?}").contains("private"));
    }
    let accepted = OutcomeSummary::of(
        "evt",
        ConsumerId::default_consumer(),
        &LaunchOutcome::Accepted {
            response_id: "private-response".into(),
        },
        Duration::MAX,
    );
    assert_eq!(accepted.accepted_ms, Some(u32::MAX));
    assert!(!format!("{accepted:?}").contains("private-response"));
}

#[test]
fn routed_summary_bounds_targets_and_counts_and_hashes_causation() {
    use cmaeg_core::activity::{ActivityRouting, Causation, ConsumerTarget, RouteResolution};
    let mut activity: Activity = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/activity.json"
    ))
    .unwrap();
    let item = serde_json::from_value(
        serde_json::json!({"reference":"private-reference", "kind":"audio"}),
    )
    .unwrap();
    activity.kind = ActivityKind::Message {
        text: "private-text".into(),
        attachments: vec![item; 300],
        mentions_bot: false,
        reply_to: None,
        album_id: None,
    };
    activity.routing = Some(ActivityRouting {
        source_id: cmaeg_core::channel::SourceId::agent(),
        resolution: RouteResolution::AgentEvent,
        rule_id: None,
        targets: vec![
            ConsumerTarget {
                id: ConsumerId::default_consumer(),
                name: cmaeg_core::ids::ConsumerName::parse("lead").unwrap()
            };
            10
        ],
        addressing_bytes: 0,
        causation: Some(Causation {
            parent_event_id: "private-parent".parse().unwrap(),
            root: "private-root".into(),
            published_by: ConsumerId::default_consumer(),
            hop: 1,
            reply: cmaeg_core::channel::ReplyChoice::Origin,
            origin_source_id: cmaeg_core::channel::SourceId::parse("custom").unwrap(),
        }),
    });
    let summary = ActivitySummary::of(&activity);
    assert_eq!(summary.attachments, [0, 255, 0, 0]);
    assert_eq!(summary.routing.as_ref().unwrap().targets.len(), 8);
    assert_eq!(summary.causation.as_ref().unwrap().parent_key.len(), 16);
    assert!(!format!("{summary:?}").contains("private"));
}
