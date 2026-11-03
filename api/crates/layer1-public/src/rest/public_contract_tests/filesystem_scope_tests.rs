use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt as _;

async fn request(
    router: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    request_for(router, "owner-a", method, path, body).await
}

async fn request_for(
    router: &Router,
    subject: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(crate::rest::agents_tests::request(
            method, path, subject, None, body,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn public_files_hide_private_agent_homes_without_withdrawing_shared_workspace_files() {
    let runtime = Arc::new(super::runtime::Files::new("owner"));
    runtime.entries.lock().unwrap().extend([
        ("cma/.cma".into(), None),
        ("cma/.cma/agents".into(), None),
        (
            "cma/.cma/agents/agt_other/home/history.jsonl".into(),
            Some(b"other Agent private transcript".to_vec()),
        ),
        ("cma/project".into(), None),
        ("cma/project/.worktrees".into(), None),
        (
            "cma/project/.worktrees/other-task/README.md".into(),
            Some(b"authorized workspace source".to_vec()),
        ),
    ]);
    let fixture = super::scenario_with_runtime("filesystem_owner", runtime)
        .await
        .unwrap();
    let private = "cma/.cma/agents/agt_other/home/history.jsonl";
    let (status, _) = request(
        &fixture.router,
        Method::GET,
        &format!("/api/v1/agents/agt_shared_read/files/content?path={private}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, bytes) = request(
        &fixture.router,
        Method::GET,
        "/api/v1/agents/agt_shared_read/files/tree?path=cma",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listing: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!listing.to_string().contains(".cma"));
    assert!(listing.to_string().contains("cma/project"));
    for endpoint in ["tree", "download"] {
        let (status, _) = request(
            &fixture.router,
            Method::GET,
            &format!("/api/v1/agents/agt_shared_read/files/{endpoint}?path={private}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _) = request(
        &fixture.router,
        Method::POST,
        "/api/v1/agents/agt_shared_read/files",
        Some(json!({"path": private, "content_base64": "eA=="})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for path in [private, "cma/.cma", "cma"] {
        let (status, _) = request(
            &fixture.router,
            Method::DELETE,
            &format!("/api/v1/agents/agt_shared_read/files?path={path}&recursive=true"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, bytes) = request(
        &fixture.router,
        Method::GET,
        "/api/v1/agents/agt_shared_read/files/download?path=cma/project/.worktrees/other-task/README.md",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"authorized workspace source");
    let (status, _) = request(
        &fixture.router,
        Method::POST,
        "/api/v1/agents/agt_shared_read/files",
        Some(json!({"path": "cma/project/new.txt", "content_base64": "ZWRpdGVk"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, bytes) = request(
        &fixture.router,
        Method::GET,
        "/api/v1/agents/agt_shared_read/files/download?path=cma/project/new.txt",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"edited");
}

#[tokio::test]
async fn public_files_scope_uploads_to_the_authorized_agent_for_owner_reader_and_writer() {
    for kind in ["owner", "viewer", "writer"] {
        let runtime = Arc::new(super::runtime::Files::new(kind));
        runtime.entries.lock().unwrap().extend([
            ("attachments".into(), None),
            ("attachments/agt_shared_read".into(), None),
            ("attachments/agt_other".into(), None),
            ("attachments/agt_shared_read_suffix".into(), None),
            (
                "attachments/agt_shared_read/upload/notes.txt".into(),
                Some(b"this Agent upload".to_vec()),
            ),
            (
                "attachments/agt_other/upload/notes.txt".into(),
                Some(b"other Agent upload".to_vec()),
            ),
        ]);
        let fixture = super::scenario_with_runtime(&format!("filesystem_{kind}"), runtime)
            .await
            .unwrap();
        let subject = fixture.subject.unwrap();
        let (status, bytes) = request_for(
            &fixture.router,
            subject,
            Method::GET,
            "/api/v1/agents/agt_shared_read/files/tree?path=attachments",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let listing: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(listing["entries"].as_array().unwrap().len(), 1);
        assert_eq!(listing["entries"][0]["name"], "agt_shared_read");
        for endpoint in ["content", "download", "tree"] {
            let (status, _) = request_for(
                &fixture.router,
                subject,
                Method::GET,
                &format!("/api/v1/agents/agt_shared_read/files/{endpoint}?path=attachments/agt_other/upload/notes.txt"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (status, bytes) = request_for(
            &fixture.router,
            subject,
            Method::GET,
            "/api/v1/agents/agt_shared_read/files/download?path=attachments/agt_shared_read/upload/notes.txt",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, b"this Agent upload");
        if kind == "viewer" {
            continue;
        }
        for path in [
            "attachments",
            "attachments/agt_other",
            "attachments/agt_other/upload/notes.txt",
        ] {
            let (status, _) = request_for(
                &fixture.router,
                subject,
                Method::DELETE,
                &format!("/api/v1/agents/agt_shared_read/files?path={path}&recursive=true"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        for path in [
            "attachments/agt_shared_read/new.txt",
            "cma/workspace-edit.txt",
        ] {
            let (status, _) = request_for(
                &fixture.router,
                subject,
                Method::POST,
                "/api/v1/agents/agt_shared_read/files",
                Some(json!({"path": path, "content_base64": "c2NvcGVk"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let (status, bytes) = request_for(
                &fixture.router,
                subject,
                Method::GET,
                &format!("/api/v1/agents/agt_shared_read/files/download?path={path}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(bytes, b"scoped");
        }
        let (status, _) = request_for(
            &fixture.router,
            subject,
            Method::POST,
            "/api/v1/agents/agt_shared_read/files",
            Some(json!({"path": "attachments/agt_other/new.txt", "content_base64": "eA=="})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
