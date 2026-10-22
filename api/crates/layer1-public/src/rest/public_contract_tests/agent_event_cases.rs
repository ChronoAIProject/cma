use super::Scenario;
use crate::agent_v2::fixtures::{append, context, fixture, unavailable, with_live_frames};
use axum::Extension;
use layer2_core::{
    resource_access::*,
    sandbox::{store::AgentStore, AgentLogKind},
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
        "lagged"
    ]
    .contains(&mode));
    let rich = matches!(mode, "owner" | "shared");
    let f = fixture(rich || matches!(mode, "live" | "lagged"), mode == "enabled").await;
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
            },
            AgentLogKind::ToolCallUpdated {
                call_id: "tool-contract".into(),
                status: "completed".into(),
                title: None,
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
