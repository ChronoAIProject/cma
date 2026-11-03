use super::Scenario;
use crate::responses::v2::binding_tests as conversation;
use crate::rest::bindings_tests as management;
use axum::Extension;
use layer2_core::{
    profiles::content_digest,
    resource_access::CallerContext,
    sandbox::binding::*,
    Authorization,
};
use serde_json::{json, Value};
use tower::ServiceExt;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
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
    if ["ready", "pending", "skill_version"].contains(&mode) {
        let body = if mode == "skill_version" {
            skill_version_body("1.0")
        } else {
            management::body()
        };
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
        if matches!(mode, "ready" | "skill_version") {
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

fn skill_version_body(version: &str) -> Value {
    let mut body = management::body();
    body["profile"]["context"]["skills"] = json!([{
        "id": "research",
        "source": "ornn",
        "version": version,
    }]);
    let digest = content_digest(&body["profile"]).unwrap();
    body["profile_digest"] = json!(format!("sha256:{digest}"));
    body
}
