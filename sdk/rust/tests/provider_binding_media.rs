#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, provider_bindings::*};
use serde_json::{Value, json};
use wire::WireServer;

fn target() -> BindingMediaTarget<'static> {
    BindingMediaTarget {
        binding_id: "bnd_one",
        conversation_id: "conv_one",
        event_id: "event_one",
        attachment_index: 0,
    }
}
fn params() -> BindingMediaUpload {
    BindingMediaUpload {
        name: "voice.ogg".into(),
        sha256: "b".repeat(64),
        event_expires_at: 1_800_000_000,
        gateway_binding_generation: None,
        media_type: "audio/ogg".into(),
    }
}
fn path(suffix: &str) -> String {
    format!(
        "/api/v1/cmaeg/bindings/bnd_one/conversations/conv_one/events/event_one/media/0/{suffix}"
    )
}
fn receipt() -> Value {
    let id = format!("att_{}", "a".repeat(64));
    json!({"id":id,"agent_id":"agt_one","name":"voice.ogg",
        "path":format!("/home/attachments/agt_one/{id}/voice.ogg"),"size_bytes":3,
        "media_type":"audio/ogg","sha256":"b".repeat(64)})
}
fn upload(status: u16, response: Value) -> wire::Expected {
    let url = format!(
        "{}?name=voice.ogg&sha256={}&event_expires_at=1800000000",
        path("attachment"),
        "b".repeat(64)
    );
    let mut expected = wire::json("POST", &url, None, status, response, true);
    expected
        .headers
        .insert("content-type".into(), "audio/ogg".into());
    expected.raw_body = Some(vec![0, 255, 10]);
    expected
}
#[tokio::test]
async fn retry_uses_same_original_slot_bytes_and_receipt_without_new_command_key() {
    let server = WireServer::new().await;
    let mut unavailable = upload(503, json!({"error":"media_continuation_unavailable"}));
    unavailable
        .response_headers
        .insert("retry-after".into(), "2".into());
    server.expect(unavailable);
    let error = server
        .client
        .provider_bindings()
        .upload_event_attachment(&target(), &params(), vec![0, 255, 10])
        .await
        .unwrap_err();
    assert!(
        matches!(error,Error::Api {status:503,code:Some(ref code),retry_after:Some(delay),..}
        if code=="media_continuation_unavailable" && delay.as_secs()==2)
    );
    assert_eq!(server.request_count(), 1);
    server.expect(upload(201, receipt()));
    let result = server
        .client
        .provider_bindings()
        .upload_event_attachment(&target(), &params(), vec![0, 255, 10])
        .await
        .unwrap();
    assert_eq!(result.sha256, params().sha256);
    server.expect(wire::json(
        "GET",
        &path("attachment"),
        None,
        200,
        receipt(),
        true,
    ));
    let stored = server
        .client
        .provider_bindings()
        .event_attachment(&target())
        .await
        .unwrap();
    assert_eq!(result.path, stored.path);
    server.assert_drained();
}
#[tokio::test]
async fn supplement_retains_exact_text_optional_generation_and_separate_response() {
    let server = WireServer::new().await;
    let base = json!({"event_expires_at":1800000000,"artifact":{"attachment_index":0,"kind":"transcript",
        "content_sha256":"c".repeat(64),"source_sha256":"b".repeat(64),"name":"voice.ogg",
        "media_type":"audio/ogg","size_bytes":3},"derived_text":"  exact transcript\n中文\n"});
    for generation in [Value::Null, json!(7)] {
        let mut value = base.clone();
        value["gateway_binding_generation"] = generation;
        let body: BindingMediaSupplement = serde_json::from_value(value.clone()).unwrap();
        server.expect(wire::json("POST",&path("supplement"),Some(value),202,
            json!({"command_id":"cmd_supplement","response_id":"resp_supplement",
                "response_url":"/api/v2/responses/resp_supplement","status":"queued",
                "queue_deadline":"2026-10-30T00:00:00Z","metadata":{"cma_parent_command_id":"cmd_original"}}),true));
        let result = server
            .client
            .provider_bindings()
            .supplement_event_media(&target(), &body)
            .await
            .unwrap();
        assert_eq!(result.response_id, "resp_supplement");
        assert_eq!(result.metadata["cma_parent_command_id"], "cmd_original");
    }
    let omitted: BindingMediaSupplement = serde_json::from_value(base).unwrap();
    assert_eq!(omitted.gateway_binding_generation, None);
    server.assert_drained();
}
#[tokio::test]
async fn forged_attachment_receipts_and_unknown_string_errors_remain_protocol_failures() {
    let server = WireServer::new().await;
    for (field, value) in [
        ("path", json!("/tmp/file")),
        ("id", json!("wrong")),
        ("size_bytes", json!(4)),
        ("sha256", json!("c".repeat(64))),
        ("media_type", json!("image/png")),
    ] {
        let mut answer = receipt();
        answer[field] = value;
        server.expect(upload(201, answer));
        assert!(
            matches!(
                server
                    .client
                    .provider_bindings()
                    .upload_event_attachment(&target(), &params(), vec![0, 255, 10])
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{field}"
        );
    }
    for answer in [
        json!({"error":"unknown"}),
        json!({"error":"media_continuation_unavailable","extra":true}),
    ] {
        server.expect(wire::json(
            "GET",
            &path("attachment"),
            None,
            503,
            answer,
            true,
        ));
        assert!(matches!(
            server
                .client
                .provider_bindings()
                .event_attachment(&target())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}
#[tokio::test]
async fn malformed_upload_authority_fails_before_transport() {
    let server = WireServer::new().await;
    for invalid in [
        BindingMediaUpload {
            name: "../escape".into(),
            ..params()
        },
        BindingMediaUpload {
            sha256: "wrong".into(),
            ..params()
        },
        BindingMediaUpload {
            event_expires_at: 0,
            ..params()
        },
        BindingMediaUpload {
            gateway_binding_generation: Some(0),
            ..params()
        },
    ] {
        assert!(matches!(
            server
                .client
                .provider_bindings()
                .upload_event_attachment(&target(), &invalid, vec![])
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
}
