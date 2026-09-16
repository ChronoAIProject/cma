use super::support_tests::*;
use axum::http::{Method, StatusCode};
use layer2_core::resource_access::{CallerContext, CredentialContext, ResourceActor};
use layer2_core::trigger_domain::LaunchStore;
use layer2_core::{Caller, LaunchId, SubjectId};

fn context(api: bool) -> CallerContext {
    CallerContext {
        caller: Caller::new(SubjectId::new(VISITOR), visitor_authority()),
        credential: CredentialContext {
            issuer: "https://nyx.example".into(),
            subject: SubjectId::new(VISITOR),
            credential_id: "verified-launch-request".into(),
        },
        actor: if api {
            ResourceActor::Api {
                subject: SubjectId::new(VISITOR),
                delegation_id: None,
            }
        } else {
            ResourceActor::Human {
                subject: SubjectId::new(VISITOR),
            }
        },
        memberships: vec![],
    }
}

#[tokio::test]
async fn launch_route_captures_verified_human_and_api_actor_without_new_public_fields() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "actor-trigger").await;
    let public = publish_trigger(&app, trigger["id"].as_str().unwrap(), "actor-publish").await;
    let path = format!(
        "/api/v1/triggers/public/{}/launches",
        public["public_id"].as_str().unwrap()
    );
    for api in [false, true] {
        let identity = context(api);
        let mut request = visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some(if api { "actor-api" } else { "actor-human" }),
        );
        request.extensions_mut().insert(identity.clone());
        let (status, body) = json_response(&app.router, request).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert!(body.get("initiation").is_none() && body.get("launch_initiation").is_none());
        let stored = app
            .launches
            .launch(
                &LaunchId(body["id"].as_str().unwrap().into()),
                &SubjectId::new(VISITOR),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.initiation.unwrap().actor(), &identity.actor);
    }
    let mut invalid = context(false);
    invalid.actor = ResourceActor::Human {
        subject: SubjectId::new("foreign"),
    };
    let mut request = visitor_request(
        Method::POST,
        &path,
        Some(launch_body(false)),
        Some("actor-invalid"),
    );
    request.extensions_mut().insert(invalid);
    assert_eq!(
        raw_response(&app.router, request).await.0,
        StatusCode::UNAUTHORIZED
    );
    let mut body = launch_body(false);
    body.as_object_mut().unwrap().insert(
        "launch_initiation".into(),
        serde_json::json!({"actor":{"kind":"human","subject":"foreign"}}),
    );
    let request = visitor_request(Method::POST, &path, Some(body), Some("actor-forged-body"));
    assert_eq!(
        raw_response(&app.router, request).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}
