//! Real in-memory Trigger, Launch, and connection fixture scenarios.

use super::super::trigger_routes_tests::support_tests::{
    create_trigger, publish_trigger, publisher_authority, visitor_authority, TestApp, OWNER,
    VISITOR,
};
use super::Scenario;
use layer2_core::{trigger_domain::ServiceRequirementState, Authorization};
use std::collections::BTreeMap;
mod faults;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if let Some(scenario) = faults::scenario(name).await {
        return Some(scenario);
    }
    match name {
        "trigger_owner" => {
            let app = TestApp::new().await;
            Some(Scenario {
                router: app.router,
                subject: Some(OWNER),
                authorization: publisher_authority(),
                symbols: BTreeMap::new(),
            })
        }
        "trigger_anonymous" => {
            let app = TestApp::new().await;
            Some(Scenario {
                router: app.router,
                subject: None,
                authorization: Authorization::new([]),
                symbols: BTreeMap::new(),
            })
        }
        "trigger_forbidden" => {
            let app = TestApp::new().await;
            Some(Scenario {
                router: app.router,
                subject: Some(OWNER),
                authorization: Authorization::new([]),
                symbols: BTreeMap::new(),
            })
        }
        "launch_review" | "launch_ready" | "launch_repeat" | "launch_quota" => {
            let app = TestApp::new().await;
            if name == "launch_ready" {
                app.broker
                    .set_requirement(ServiceRequirementState::Satisfied);
            }
            if name == "launch_quota" {
                app.quota.refuse();
            }
            let trigger = create_trigger(
                &app,
                if name == "launch_repeat" {
                    "ask"
                } else {
                    "always_new"
                },
                "contract-setup-trigger",
            )
            .await;
            let published = publish_trigger(
                &app,
                trigger["id"].as_str().unwrap(),
                "contract-setup-publish",
            )
            .await;
            if name == "launch_repeat" {
                app.set_existing_agent(trigger["id"].as_str().unwrap());
            }
            Some(Scenario {
                router: app.router,
                subject: Some(VISITOR),
                authorization: visitor_authority(),
                symbols: BTreeMap::from([
                    ("trigger_id".into(), trigger["id"].clone()),
                    ("public_id".into(), published["public_id"].clone()),
                ]),
            })
        }
        _ => None,
    }
}
