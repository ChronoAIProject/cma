use super::Scenario;
use crate::responses::v2::binding_tests as conversation;
use crate::rest::bindings_tests as management;
use axum::Extension;
use layer2_core::{resource_access::CallerContext, sandbox::binding::*, Authorization};
use serde_json::json;
use tower::ServiceExt;

mod capacity;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if let Some(mode) = name.strip_prefix("binding_profile_transfer_") {
        use capacity::TransferOutcome;
        let outcome = match mode {
            "reserved" | "anonymous" | "forbidden" | "invalid" => TransferOutcome::Reserved,
            "missing" => TransferOutcome::Missing,
            "conflict" => TransferOutcome::Conflict,
            "unavailable" => TransferOutcome::Unavailable,
            _ => return None,
        };
        return Some(Scenario {
            router: capacity::profile_transfer_fixture(outcome),
            subject: (mode != "anonymous").then_some("manager"),
            authorization: if mode == "forbidden" {
                Authorization::none()
            } else {
                management::authority()
            },
            symbols: Default::default(),
        });
    }
    if name == "binding_management_capacity" {
        return Some(Scenario {
            router: capacity::fixture(),
            subject: Some("manager"),
            authorization: management::authority(),
            symbols: Default::default(),
        });
    }
    if let Some(mode) = name.strip_prefix("binding_reset_") {
        let (router, _, _, _) = management::reset::fixture().await;
        return Some(Scenario {
            router,
            subject: if mode == "anonymous" {
                None
            } else if mode == "foreign" {
                Some("foreign-manager")
            } else {
                Some("manager")
            },
            authorization: if mode == "forbidden" {
                Authorization::none()
            } else {
                management::authority()
            },
            symbols: Default::default(),
        });
    }
    if let Some(mode) = name.strip_prefix("worker_classifier_") {
        return Some(Scenario {
            router: crate::rest::worker_credentials::tests::fixture(mode).await,
            subject: None,
            authorization: Authorization::none(),
            symbols: Default::default(),
        });
    }
    if let Some(mode) = name.strip_prefix("binding_preparation_") {
        return Some(Scenario {
            router: management::preparation_contract::fixture(mode).await,
            subject: (mode != "anonymous").then_some("manager"),
            authorization: if mode == "forbidden" {
                Authorization::none()
            } else {
                management::authority()
            },
            symbols: Default::default(),
        });
    }
    if name == "provider_card" {
        return Some(Scenario {
            router: crate::a2a::router("https://cma.example"),
            subject: None,
            authorization: Authorization::none(),
            symbols: Default::default(),
        });
    }
    if name == "binding_event" {
        let (app, _, _, principal) = conversation::fixture().await;
        let context = json!({"schema_version":1,"channel_id":"channel-contract","conversation_id":"conversation-external",
            "activity":{"schema_version":1,"event_id":"event-contract","kind":{"type":"message","text":"Hello"}},"event_ref":"context-reference"});
        let request = conversation::request(
            "/api/v2/responses",
            &principal,
            json!({"conversation":"conversation-external","stream":true,"input":"Hello","event_context":context}),
        );
        let caller = request.extensions().get::<CallerContext>().unwrap().clone();
        let credential = request
            .extensions()
            .get::<crate::DelegatedCredential>()
            .unwrap()
            .clone();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
        drop(response);
        return Some(Scenario {
            router: app.layer(Extension(caller)).layer(Extension(credential)),
            subject: Some("owner-a"),
            authorization: principal.caller().authorization,
            symbols: Default::default(),
        });
    }
    let mode = name.strip_prefix("binding_management_")?;
    let (router, store) = management::app();
    if ["ready", "pending", "conversation"].contains(&mode) {
        let body = management::body();
        let response = router
            .clone()
            .oneshot(management::request(
                "PUT",
                "/api/v1/cmaeg/bindings/binding-contract",
                Some(body),
                Some(management::authority()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        if mode != "pending" {
            let mut binding = store.binding("binding-contract").await.unwrap().unwrap();
            let revision = binding.revision;
            binding.state = BindingState::Ready;
            binding.work = BindingWork::Idle;
            binding
                .advance_revision(layer2_core::now_rfc3339())
                .unwrap();
            management::create_backing_resources(&store, &binding).await;
            store.save_binding(Some(revision), binding).await.unwrap();
        }
        if mode == "conversation" {
            let response = router
                .clone()
                .oneshot(management::request(
                    "PUT",
                    "/api/v1/cmaeg/bindings/binding-contract/conversations/conversation-contract",
                    None,
                    Some(management::authority()),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 201);
        }
    }
    Some(Scenario {
        router,
        subject: (mode != "anonymous").then_some("manager"),
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            management::authority()
        },
        symbols: Default::default(),
    })
}
