use super::*;
use axum::{body::Body, http::Request};
use serde_json::{json, Value};
use tower::ServiceExt;

impl ReuseCommandFixture {
    pub async fn reply_mid_turn(
        &self,
        queued: &Queued,
    ) -> Arc<crate::store::MongoBridgeReplyToolStore> {
        let f = &self.f.original;
        let agent = &queued.command.receipt.envelope.agent_id;
        let credential = f.tools.fixture_credential(agent).await;
        let replies = Arc::new(
            crate::store::MongoBridgeReplyToolStore::new(
                queued.store.clone(),
                Arc::new(connections::Connections(f.resources.clone())),
            )
            .await
            .unwrap(),
        );
        let router = crate::bridge::channel_tools::router(
            Arc::new(crate::bridge::channel_tools::StoredChannelTools {
                capabilities: f.tools.clone(),
                replies: replies.clone(),
            }),
            "https://cma.example",
        );
        let call = |name: &'static str, arguments: Value| {
            let router = router.clone();
            let credential = credential.clone();
            async move {
                let response = router
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/api/v1/channel-tools")
                            .header("content-type", "application/json")
                            .header("authorization", format!("Bearer {}", credential.expose()))
                            .body(Body::from(
                                json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                        "params":{"name":name,"arguments":arguments}})
                                .to_string(),
                            ))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), axum::http::StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 32_768)
                    .await
                    .unwrap();
                let result: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(result["result"]["isError"], false, "{result}");
                result["result"]["structuredContent"].clone()
            }
        };
        let context = call("read_channel_context", json!({})).await;
        let reply_context = context["reply_context"].as_str().unwrap();
        let result = call(
            "reply_to_channel",
            json!({
                "reply_context":reply_context, "request_key":"reused-child-progress-1",
                "text":"The parent is awake; I am continuing the requested work.",
            }),
        )
        .await;
        assert!(result.is_object());
        let command = f
            .resources
            .get_command(&queued.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.status, AgentResponseStatus::InProgress);
        assert_eq!(command.receipt, queued.command.receipt);
        assert_eq!(
            f.client
                .database(&f.database)
                .collection::<Document>("bridge_outbox_items")
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        replies
    }
}
