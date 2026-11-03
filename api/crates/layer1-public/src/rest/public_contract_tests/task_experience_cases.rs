use super::Scenario;
use crate::agent_v2::fixtures::{context, fixture};
use axum::Extension;
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("task_policy_")?;
    let fixture = fixture(false, true).await;
    let (selection, accounts, capabilities) = match mode {
        "cohort" | "excluded" => (
            "accounts",
            vec!["owner-a"],
            vec!["task_home_admission", "replay_presentation"],
        ),
        "empty" => ("accounts", vec![], vec!["desktop_enable"]),
        "all" => (
            "all",
            vec![],
            vec![
                "task_home_admission",
                "suggested_followup_admission",
                "desktop_enable",
                "browser_takeover",
                "replay_presentation",
            ],
        ),
        "disabled" | "anonymous" => ("disabled", vec![], vec![]),
        _ => panic!("unknown task policy scenario"),
    };
    let policy = Arc::new(
        serde_json::from_value(
            serde_json::json!({"mode":selection,"accounts":accounts,"capabilities":capabilities}),
        )
        .unwrap(),
    );
    let subject = if mode == "excluded" || mode == "all" {
        "owner-b"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    let router = crate::agent_v2::router(fixture.state.with_task_experience(policy))
        .layer(Extension(caller.clone()));
    Some(Scenario {
        router,
        subject: (mode != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: Default::default(),
    })
}
