//! Receipt aliases must retain the same tenant boundary as their parent routes.
use axum::{body::to_bytes, http::Method, Router};
use layer2_core::sandbox::control::SandboxControlStore;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn sandbox_read(app: &Router, path: &str, subject: &str, head: bool) -> (u16, Value) {
    let mut request = crate::rest::agents_tests::request(
        if head { Method::HEAD } else { Method::GET },
        path,
        subject,
        None,
        None,
    );
    let context = crate::rest::sandbox_commands_tests::context(subject);
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(crate::DelegatedCredential::new("request-credential".into()));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    let value = if head {
        assert!(bytes.is_empty());
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

#[tokio::test]
async fn tenant_isolation_sandbox_receipts_hide_foreign_ids_and_recheck_revocation() {
    let f = crate::rest::sandbox_commands_tests::Fixture::new().await;
    let record = f.admit("recipient").await;
    let path = format!("/api/v1/sandbox-commands/{}", record.envelope.command_id);
    let absent = "/api/v1/sandbox-commands/scmd_absent";
    for head in [false, true] {
        assert_eq!(
            sandbox_read(&f.router, &path, "recipient", head).await.0,
            200
        );
        let denied = sandbox_read(&f.router, &path, "foreign", head).await;
        assert_eq!(denied.0, 404);
        assert_eq!(
            denied,
            sandbox_read(&f.router, absent, "foreign", head).await
        );
    }
    f.revoke().await;
    for head in [false, true] {
        let denied = sandbox_read(&f.router, &path, "recipient", head).await;
        assert_eq!(denied.0, 404);
        if !head {
            assert!(denied.1.get("command_id").is_none());
            assert!(denied.1.get("sandbox_id").is_none());
        }
    }
    assert_eq!(
        f.store
            .sandbox_control(&record.envelope.command_id)
            .await
            .unwrap(),
        Some(record)
    );
}

#[tokio::test]
async fn tenant_isolation_v2_command_receipts_hide_foreign_and_absent_targets() {
    use crate::responses::v2::controls::fixtures::*;
    use layer2_core::sandbox::agent_io::AgentCommandStore;
    let f = fixture(true).await;
    let body = intent(&f, json!({"type":"cancel"})).await;
    let (status, accepted) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("isolation"),
        Some(body),
    )
    .await;
    assert_eq!(status.as_u16(), 202);
    let id = accepted["control"]["receipt"]["command_id"]
        .as_str()
        .unwrap();
    let path = format!("/api/v2/commands/{id}");
    let stored = f
        .store
        .get_command(&layer2_core::ids::AgentCommandId(id.into()))
        .await
        .unwrap();
    for head in [false, true] {
        let method = if head { Method::HEAD } else { Method::GET };
        let mut denied = None;
        for (subject, target, expected) in [
            ("owner-a", path.as_str(), 200),
            ("foreign", path.as_str(), 404),
            ("foreign", "/api/v2/commands/cmd_absent", 404),
        ] {
            let response = f
                .router
                .clone()
                .oneshot(request(method.clone(), target, subject, None, None))
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), expected);
            let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
            if head {
                assert!(bytes.is_empty());
            }
            if expected == 404 {
                if let Some(previous) = &denied {
                    assert_eq!(previous, &bytes);
                }
                denied = Some(bytes);
            }
        }
    }
    assert_eq!(
        f.store
            .get_command(&layer2_core::ids::AgentCommandId(id.into()))
            .await
            .unwrap(),
        stored
    );
}
