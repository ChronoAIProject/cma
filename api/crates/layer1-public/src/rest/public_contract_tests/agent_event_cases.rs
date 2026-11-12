use super::Scenario;
use crate::agent_v2::fixtures::{append, context, fixture, unavailable, with_live_frames};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{store::AgentStore, AgentLogKind, AgentPlanStep, RecordedToolOutput},
    AgentId, SubjectId,
};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("agent_events_v2_")?;
    assert!([
        "owner",
        "shared",
        "foreign",
        "anonymous",
        "corrupt",
        "head_corrupt",
        "unavailable",
        "enabled",
        "off",
        "live",
        "lagged",
        "operation_replay"
    ]
    .contains(&mode));
    let rich = matches!(mode, "owner" | "shared");
    let f = fixture(
        rich || matches!(mode, "live" | "lagged" | "operation_replay"),
        mode == "enabled",
    )
    .await;
    if mode == "operation_replay" {
        append(
            &f.store,
            AgentLogKind::PlanSet {
                entries: vec![AgentPlanStep {
                    title: "Inspect recorded commands".into(),
                    status: "in_progress".into(),
                }],
            },
        )
        .await;
        for number in 1..=3 {
            append(
                &f.store,
                AgentLogKind::ToolCallStarted {
                    call_id: format!("replay-{number}"),
                    title: format!("printf replay-{number}"),
                    kind: "execute".into(),
                    status: "running".into(),
                },
            )
            .await;
            append(
                &f.store,
                AgentLogKind::ToolCallUpdated {
                    call_id: format!("replay-{number}"),
                    status: "completed".into(),
                    title: None,
                    output: Some(RecordedToolOutput {
                        text: format!("replay-{number}\n"),
                        truncated: number == 3,
                    }),
                },
            )
            .await;
        }
        append(&f.store, AgentLogKind::PlanSet { entries: vec![] }).await;
    }
    if rich {
        for kind in [
            AgentLogKind::ToolCallStarted {
                call_id: "tool-contract".into(),
                title: "Inspect file".into(),
                kind: "read".into(),
                status: "running".into(),
            },
            AgentLogKind::ToolCallUpdated {
                call_id: "tool-contract".into(),
                status: "running".into(),
                title: Some("Inspect configuration".into()),
                output: None,
            },
            AgentLogKind::ToolCallUpdated {
                call_id: "tool-contract".into(),
                status: "completed".into(),
                title: None,
                output: None,
            },
            AgentLogKind::ApprovalRequested {
                approval_id: "approval-contract".into(),
                action: Some("write".into()),
                detail: Some("Update configuration".into()),
                runtime_locator: Some("private-locator".into()),
            },
            AgentLogKind::ApprovalResolved {
                approval_id: "approval-contract".into(),
                decision: "allow".into(),
            },
            AgentLogKind::Usage {
                input_tokens: 10,
                output_tokens: 20,
                context_used: 30,
                context_window: Some(128000),
            },
            AgentLogKind::AgentMessage {
                content: "Configuration reviewed.".into(),
            },
        ] {
            append(&f.store, kind).await;
        }
    }
    if matches!(mode, "corrupt" | "head_corrupt") {
        let agent = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        append(
            &f.store,
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: agent.conversation.last_seq + 1,
                origin_key: Some("command:cmd_missing".into()),
            },
        )
        .await;
    }
    if mode == "head_corrupt" {
        let mut agent = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = agent.revision;
        agent.conversation.active_turn = Some(agent.conversation.last_seq);
        agent.status = layer2_core::sandbox::AgentStatus::Working;
        agent.revision += 1;
        f.store.save_agent(revision, agent).await.unwrap();
    }
    if mode == "shared" {
        f.access
            .change_share(
                &context("owner-a"),
                Some("fixture-credential"),
                ShareCommand {
                    selector: ResourceSelector::Agent {
                        agent_id: AgentId("agt_fixture".into()),
                    },
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    idempotency_key: "event-contract-share".into(),
                },
            )
            .await
            .unwrap();
    }
    let subject = match mode {
        "shared" => "reader",
        "foreign" => "foreign",
        _ => "owner-a",
    };
    let caller = context(subject);
    let router = match mode {
        "unavailable" => unavailable(&f),
        "live" | "lagged" => with_live_frames(&f, mode == "lagged"),
        _ => f.router,
    };
    Some(Scenario {
        router: router.layer(Extension(caller.clone())).layer(Extension(
            crate::DelegatedCredential::new("fixture-credential".into()),
        )),
        subject: (mode != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}

async fn replay_live_contract(id: &str) {
    let corpus: super::Corpus = serde_json::from_str(include_str!(
        "../../../tests/fixtures/public-contract/agent-events-v2.json"
    ))
    .unwrap();
    let mut case = corpus.cases.into_iter().find(|case| case.id == id).unwrap();
    let scene = scenario(&case.scenario).await.unwrap();
    super::replay::run(&scene, &mut super::Symbols::default(), &mut case, false).await;
}

#[tokio::test]
async fn live_contract_examples_preserve_frame_order_and_explicit_lag() {
    replay_live_contract("agent-events-v2-live").await;
    replay_live_contract("agent-events-v2-lagged").await;
}

async fn replay_snapshot_with_separate_confirmation_time(id: &str, change_kind: bool) {
    let corpus: super::Corpus = serde_json::from_str(include_str!(
        "../../../tests/fixtures/public-contract/agent-events-v2.json"
    ))
    .unwrap();
    let mut case = corpus.cases.into_iter().find(|case| case.id == id).unwrap();
    let mut actual = case.response.body.clone();
    // Admission and write confirmation need not occur in the same wall-clock
    // second. Distinct fixed timestamps expose an omitted capture reliably.
    for index in 2..7 {
        actual["items"][index]["appended_at"] = "2026-11-12T06:22:29Z".into();
    }
    actual["items"][7]["appended_at"] = "2026-11-12T06:22:30Z".into();
    if change_kind {
        actual["items"][7]["kind"]["state"] = "dispatch_fenced".into();
    }
    let scene = Scenario {
        router: axum::Router::new().route(
            "/api/v2/agents/agt_fixture/events/snapshot",
            axum::routing::get(move || async move {
                ([("cache-control", "private, no-store")], axum::Json(actual))
            }),
        ),
        subject: None,
        authorization: layer2_core::Authorization::new([]),
        symbols: Default::default(),
    };
    super::replay::run(&scene, &mut super::Symbols::default(), &mut case, false).await;
}

#[tokio::test]
async fn snapshot_contracts_capture_write_confirmation_in_a_later_second() {
    for id in [
        "agent-events-v2-snapshot-rich",
        "agent-events-v2-snapshot-shared",
    ] {
        replay_snapshot_with_separate_confirmation_time(id, false).await;
    }
}

#[tokio::test]
#[should_panic(expected = "agent-events-v2-snapshot-shared response body")]
async fn timestamp_capture_does_not_mask_changed_delivery_state() {
    replay_snapshot_with_separate_confirmation_time("agent-events-v2-snapshot-shared", true).await;
}
