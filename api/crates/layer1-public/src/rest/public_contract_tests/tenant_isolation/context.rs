use super::*;
use serde_json::json;

#[tokio::test]
async fn conversation_creation_cannot_select_another_owners_parent_in_metadata() {
    for (kind, id, code) in [
        ("workspace", "wks_route", "workspace_not_found"),
        ("sandbox", "sbx_route", "sandbox_not_found"),
    ] {
        let scene = super::super::scenario("experience_owner").await;
        for (subject, expected) in [("isolation-foreign-tenant", 404), ("owner-a", 200)] {
            let mut request = Request::post("/api/v1/conversations")
                .header("content-type", "application/json")
                .body(Body::from(json!({"metadata":{kind:id}}).to_string()))
                .unwrap();
            request
                .extensions_mut()
                .insert(crate::AuthenticatedSubject(SubjectId::new(subject)));
            request
                .extensions_mut()
                .insert(Authorization::new([Capability::UseOwnSandboxes]));
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                scene.router.clone().oneshot(request),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.status().as_u16(), expected, "{kind} {subject}");
            let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            if expected == 404 {
                assert_eq!(body["code"], code);
                assert!(body.get("metadata").is_none());
            } else {
                assert_eq!(body["object"], "conversation");
                assert_eq!(body["metadata"][kind], id);
            }
        }
    }
}
