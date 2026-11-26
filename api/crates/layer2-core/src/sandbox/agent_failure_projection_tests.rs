use super::*;

const CLASSES: &[&str] = &[
    "upstream_timeout",
    "upstream_unavailable",
    "upstream_rate_limited",
    "insufficient_credits",
    "agent_turn_failed",
    "owner_budget_exhausted",
    "turn_budget_exhausted",
    "budget_unavailable",
    "platform_budget_exhausted",
    "agent_log_capacity_exceeded",
    "conversation_output_gap",
    AGENT_TURN_NO_PROGRESS,
    "runtime_lost",
    SOURCE_DEFERRED_FAILURE,
];

#[test]
fn public_failure_projection_is_bounded_idempotent_and_never_trusts_known_details() {
    for class in CLASSES.iter().copied().chain([
        "",
        "PRIVATE_CLASS_SENTINEL",
        "agent_turn_failed\nPRIVATE_CLASS_SENTINEL",
    ]) {
        let raw = TypedFailure {
            class: class.into(),
            detail: "unexpected status 503: PRIVATE_BODY_SENTINEL".repeat(40_000),
        };
        let safe = public_agent_turn_failure(&raw);
        assert_eq!(
            safe.class,
            if CLASSES.contains(&class) {
                class
            } else {
                "agent_turn_failed"
            }
        );
        assert!(!serde_json::to_string(&safe).unwrap().contains("SENTINEL"));
        assert!(safe.detail.len() < 512);
        assert_eq!(public_agent_turn_failure(&safe), safe);
        assert!(raw.detail.contains("SENTINEL"));
    }
}

#[test]
fn old_generic_errors_stay_generic_and_budget_guidance_stays_exact() {
    for (class, detail) in [
        ("agent_turn_failed", "The Agent run failed."),
        ("upstream_timeout", "The upstream model service timed out."),
        (
            "upstream_unavailable",
            "The upstream model service is temporarily unavailable.",
        ),
        (
            "upstream_rate_limited",
            "The upstream model service is limiting requests. Wait before sending another message.",
        ),
    ] {
        let safe = public_agent_turn_failure(&TypedFailure {
            class: class.into(),
            detail: "unexpected status 503: private diagnostic".into(),
        });
        assert_eq!(safe.detail, format!("{detail} {CONTINUATION}"));
    }
    for budget in [
        BudgetError::OwnerExhausted,
        BudgetError::TurnExhausted,
        BudgetError::Unavailable,
        BudgetError::PlatformExhausted,
    ] {
        let safe = public_agent_turn_failure(&TypedFailure {
            class: budget.code().into(),
            detail: "private".into(),
        });
        assert_eq!(safe.detail, budget.guidance());
    }
}

#[test]
fn new_failure_classes_round_trip_through_the_predecessor_string_reader() {
    #[derive(serde::Deserialize, serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct PredecessorFailure {
        class: String,
        detail: String,
    }

    for class in CLASSES {
        let safe = public_agent_turn_failure(&TypedFailure {
            class: (*class).into(),
            detail: "private".into(),
        });
        let bytes = serde_json::to_vec(&safe).unwrap();
        let old: PredecessorFailure = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(old.class, *class);
        assert_eq!(old.detail, safe.detail);
        assert_eq!(serde_json::to_vec(&old).unwrap(), bytes);
        assert_eq!(
            serde_json::from_slice::<TypedFailure>(&bytes).unwrap(),
            safe
        );
        let mut value = serde_json::to_value(safe).unwrap();
        value["http_status"] = 503.into();
        assert!(serde_json::from_value::<TypedFailure>(value.clone()).is_err());
        assert!(serde_json::from_value::<PredecessorFailure>(value).is_err());
    }
}

#[test]
fn deferred_proof_stays_private_and_original_after_visible_failure_projection() {
    use crate::{
        sandbox::{AgentLogEntry, AgentLogKind, AgentVisibleLogEntry},
        AgentId,
    };
    for personal in [false, true] {
        let visible = AgentVisibleLogEntry::from_entry(
            AgentLogEntry {
                agent_id: AgentId("agent".into()),
                seq: 8,
                appended_at: "2026-11-19T00:00:00Z".into(),
                kind: AgentLogKind::TurnFailed {
                    turn_sequence: 3,
                    failure: TypedFailure {
                        class: SOURCE_DEFERRED_FAILURE.into(),
                        detail: "PRIVATE_PROOF_SENTINEL".into(),
                    },
                },
            },
            personal,
        );
        assert_eq!(
            visible.deferred_failure_detail.as_deref(),
            Some("PRIVATE_PROOF_SENTINEL")
        );
        let Some(AgentLogKind::TurnFailed { failure, .. }) = visible.kind else {
            panic!("failure")
        };
        assert_eq!(failure.class, SOURCE_DEFERRED_FAILURE);
        assert!(!failure.detail.contains("SENTINEL"));
    }
}
#[test]
fn service_timeout_and_approval_failures_use_only_fixed_public_copy() {
    for class in [
        "agent_turn_timeout",
        "approval_timeout",
        "approval_denied",
        "approval_expired",
        "approval_wait_disconnected",
    ] {
        let projected = super::public_agent_turn_failure(&super::TypedFailure {
            class: class.into(),
            detail: "PRIVATE_APPROVAL_RECEIPT_SENTINEL".into(),
        });
        assert_eq!(projected.class, class);
        assert!(!projected.detail.contains("SENTINEL"));
        assert_eq!(super::public_agent_turn_failure(&projected), projected);
    }
    let generic = super::public_agent_turn_failure(&super::TypedFailure {
        class: "agent_turn_timeout".into(),
        detail: "untrusted".into(),
    });
    assert!(!generic.detail.contains("approval"));
    assert!(!generic.detail.contains("no new progress"));
}
