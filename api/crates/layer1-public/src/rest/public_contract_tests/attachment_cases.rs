use super::*;
use crate::rest::{
    resource_access_tests::context,
    shared_agent_tests::{fixture, Reply},
};
use layer2_core::sandbox::{agent_attachments::*, Sandbox};
use std::sync::Arc;

struct Writer(String);
#[async_trait::async_trait]
impl AttachmentWriter for Writer {
    async fn write(
        &self,
        _: &Sandbox,
        attachment: &AgentAttachment,
        content: Vec<u8>,
    ) -> Result<AgentAttachment, AttachmentError> {
        if self.0 == "large" {
            assert_eq!(content, vec![0; 3 * 1024 * 1024]);
        } else {
            assert_eq!(content, b"owner file\n");
        }
        match self.0.as_str() {
            "unsupported" => Err(AttachmentError::Unsupported),
            "conflict" => Err(AttachmentError::Conflict),
            "unavailable" => Err(AttachmentError::Unavailable),
            _ => Ok(attachment.clone()),
        }
    }
}
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let kind = name.strip_prefix("attachment_")?;
    let (_, access, _, _) = fixture(Reply::Member).await;
    let service = AgentAttachmentService::with_writer(access, Arc::new(Writer(kind.into())));
    let subject = if kind == "foreign" {
        "foreign"
    } else {
        "owner-a"
    };
    let caller = context(subject);
    Some(Scenario {
        router: super::super::agent_attachments_router(service),
        subject: (kind != "anonymous").then_some(subject),
        authorization: caller.caller.authorization,
        symbols: BTreeMap::new(),
    })
}

#[tokio::test]
async fn attachment_route_accepts_binary_above_the_ordinary_body_limit_and_enforces_its_own_bound()
{
    use axum::{
        body::{to_bytes, Body},
        extract::DefaultBodyLimit,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let scenario = scenario("attachment_large").await.unwrap();
    let router = scenario
        .router
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024));
    for (size, status) in [
        (3 * 1024 * 1024, StatusCode::CREATED),
        (MAX_ATTACHMENT_BYTES + 1, StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let mut request =
            Request::post("/api/v1/agents/agt_shared_read/attachments?name=binary.dat")
                .header("idempotency-key", "large-binary")
                .header("content-type", "application/octet-stream")
                .body(Body::from(vec![0; size]))
                .unwrap();
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(layer2_core::SubjectId(
                "owner-a".into(),
            )));
        request
            .extensions_mut()
            .insert(scenario.authorization.clone());
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        if status == StatusCode::CREATED {
            assert_eq!(body["size_bytes"], size);
        } else {
            assert_eq!(body["code"], "attachment_too_large");
        }
    }
}
