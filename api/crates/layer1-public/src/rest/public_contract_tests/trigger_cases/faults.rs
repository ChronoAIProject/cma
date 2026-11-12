use std::collections::BTreeMap;

use axum::http::{Method, StatusCode};
use layer2_core::trigger_domain::{
    BrokerHandoffState, LaunchPolicy, LaunchServiceError, LaunchStoreError, ServiceConnectionError,
    ServiceRequirementState, Trigger, TriggerError, TriggerId, TriggerStore, TriggerStoreError,
};
use layer2_core::{OperationReservation, StoreError};
use serde_json::json;

use super::super::super::trigger_routes_tests::support_tests::*;
use super::super::Scenario;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let (domain, mode) = name.split_once("_fault_")?;
    if !matches!(domain, "trigger" | "launch") {
        return None;
    }
    let app = if mode == "private_reference" {
        TestApp::private_profiles().await
    } else {
        TestApp::new().await
    };
    let visitor = domain == "launch";
    let mut symbols = BTreeMap::new();
    if mode == "store_unavailable" || mode == "store_corrupt" {
        let detail = "contract storage fault".to_owned();
        if visitor {
            app.launches.fail(
                "*",
                if mode == "store_unavailable" {
                    LaunchStoreError::Unavailable(detail)
                } else {
                    LaunchStoreError::Corrupt(detail)
                },
            );
        } else {
            app.definitions.fail(
                "*",
                if mode == "store_unavailable" {
                    TriggerStoreError::Unavailable(detail)
                } else {
                    TriggerStoreError::Corrupt(detail)
                },
            );
        }
    } else if mode == "duplicate" && !visitor {
        app.definitions
            .fail("create_trigger", TriggerStoreError::Duplicate);
    } else if mode == "private_reference" && !visitor {
        // Fixture requests create and publish the draft against real private profile state.
    } else if mode == "version_overflow" && !visitor {
        let mut trigger = Trigger::new(
            TriggerId("trg_contract".into()),
            layer2_core::SubjectId::new(OWNER),
            serde_json::from_value(trigger_body("always_new")["content"].clone()).unwrap(),
            "2026-09-07T00:00:00Z".into(),
        )
        .unwrap();
        trigger.draft_version = u64::MAX;
        app.definitions.create_trigger(trigger).await.unwrap();
    } else {
        let trigger = create_trigger(&app, "always_new", "contract-seed-trigger").await;
        symbols.insert("trigger_id".into(), trigger["id"].clone());
        if visitor {
            let published = publish_trigger(
                &app,
                trigger["id"].as_str().unwrap(),
                "contract-seed-publish",
            )
            .await;
            symbols.insert("public_id".into(), published["public_id"].clone());
            if mode == "commit_conflict" {
                app.broker
                    .set_requirement(ServiceRequirementState::Satisfied);
            }
            let (status, launch) = json_response(
                &app.router,
                visitor_request(
                    Method::POST,
                    &format!(
                        "/api/v1/triggers/public/{}/launches",
                        published["public_id"].as_str().unwrap()
                    ),
                    Some(json!({"start_if_ready":false})),
                    Some("contract-seed-launch"),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{launch}");
            symbols.insert("launch_id".into(), launch["id"].clone());
            if matches!(mode, "broker_unavailable" | "broker_malformed")
                || mode.starts_with("handoff_")
            {
                let (status, handoff) = json_response(&app.router, visitor_request(
                    Method::POST, "/api/v1/service-connections",
                    Some(json!({"launch_id":launch["id"],"service_id":"github","intent_id":"contract-intent"})),
                    Some("connection-create"),
                )).await;
                assert_eq!(status, StatusCode::CREATED, "{handoff}");
                symbols.insert("handoff_id".into(), handoff["handoff_id"].clone());
            }
        }
        match mode {
            "publish_revision_overflow" => {
                // Core-port exhaustion projection, not a four-billion-revision allocation.
                app.definitions.fail(
                    "publish_trigger",
                    TriggerStoreError::Validation(TriggerError::new(
                        "trigger_revision_overflow",
                        "revisions",
                    )),
                );
            }
            "operation_pending" => app.fail_operations(Ok(OperationReservation::Pending)),
            "operation_recovery" => app.fail_operations(Ok(OperationReservation::RecoveryRequired)),
            "operation_unavailable" => app.fail_operations(Err(StoreError::Unavailable(
                "contract operation fault".into(),
            ))),
            "broker_unavailable" => app.broker.fail(ServiceConnectionError::Unavailable(
                "contract broker fault".into(),
            )),
            "broker_malformed" => app.broker.fail(ServiceConnectionError::Malformed(
                "contract broker data fault".into(),
            )),
            "policy_unavailable" => app.policy.set(Err(LaunchServiceError::Unavailable(
                "contract policy fault".into(),
            ))),
            "disabled" => app.policy.set(Ok(LaunchPolicy {
                enabled: false,
                orchestration_paused: false,
                visitor_launches_per_day: 5,
            })),
            "quota_unavailable" => app.quota.unavailable(),
            "quota_refused" => app.quota.refuse(),
            "commit_conflict" => app.launches.fail(
                "commit_launch",
                LaunchStoreError::Conflict {
                    expected: 1,
                    actual: 2,
                },
            ),
            "admit_conflict" => app.launches.fail(
                "admit_launch",
                LaunchStoreError::Conflict {
                    expected: 1,
                    actual: 2,
                },
            ),
            "connection_conflict" => app.launches.fail(
                "create_handoff",
                LaunchStoreError::Conflict {
                    expected: 1,
                    actual: 2,
                },
            ),
            "handoff_conflict" => {
                app.broker.observe(BrokerHandoffState::Satisfied);
                app.launches.fail(
                    "save_handoff",
                    LaunchStoreError::Conflict {
                        expected: 1,
                        actual: 2,
                    },
                );
            }
            "handoff_satisfied" => app.broker.observe(BrokerHandoffState::Satisfied),
            "handoff_expired" => app.broker.observe(BrokerHandoffState::Expired),
            "handoff_cancelled" => app.broker.observe(BrokerHandoffState::Cancelled),
            "handoff_failed" => app.broker.observe(BrokerHandoffState::Failed),
            "trigger_retired" => {
                let (status, _) = json_response(
                    &app.router,
                    publisher_request(
                        Method::POST,
                        &format!(
                            "/api/v1/triggers/{}/retire",
                            trigger["id"].as_str().unwrap()
                        ),
                        None,
                        Some("contract-seed-retire"),
                    ),
                )
                .await;
                assert_eq!(status, StatusCode::OK);
            }
            _ => panic!("unknown Trigger fault scenario {name}"),
        }
    }
    Some(Scenario {
        router: app.router,
        subject: Some(if visitor { VISITOR } else { OWNER }),
        authorization: if visitor {
            visitor_authority()
        } else {
            publisher_authority()
        },
        symbols,
    })
}
