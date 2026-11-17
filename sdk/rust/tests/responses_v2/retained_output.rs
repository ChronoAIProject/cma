use super::wire::{Expected, WireServer, headers};
use cma_sdk::Error;
use serde_json::{Value, json};
use std::collections::BTreeMap;
fn output() -> Value {
    json!({"id":"retained_checkpoint", "agent_id":"agent", "turn_sequence":3, "checkpoint_sequence":5, "failure_sequence":6, "failed_at":"2026-11-17T00:00:00Z", "byte_length":1024*1024, "partial":true})
}
fn expect(server: &WireServer, route: &str, body: Value, status: u16) {
    server.expect(Expected {
        method: "GET".into(),
        path: route.into(),
        headers: headers(true, "application/json"),
        status,
        response_headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        response: serde_json::to_vec(&body).unwrap(),
        ..Expected::default()
    });
}
#[tokio::test]
async fn retained_output_maximum_multibyte_prefix_uses_read_only_chunks_on_both_surfaces() {
    for response in [false, true] {
        for (text, requests) in [
            (format!("{}x", "界".repeat((1024 * 1024 - 1) / 3)), 3),
            ("\u{1}".repeat(1024 * 1024), 2),
        ] {
            let server = WireServer::new().await;
            let mut offset = 0;
            let mut actual = String::new();
            let mut cursor = None;
            let route = if response {
                "/api/v2/responses/response/retained-output"
            } else {
                "/api/v1/agents/agent/retained-output"
            };
            while offset < text.len() {
                let mut end = text.len().min(offset + 512 * 1024);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let next = (end < text.len()).then(|| format!("retained_checkpoint:{end}"));
                let uri = format!(
                    "{route}{}",
                    cursor.as_ref().map_or(String::new(), |c: &String| format!(
                        "?cursor={}",
                        c.replace(':', "%3A")
                    ))
                );
                expect(
                    &server,
                    &uri,
                    json!({"output":output(),"offset":offset,"content":&text[offset..end],"next":next,"future_field":true}),
                    200,
                );
                let page = if response {
                    server
                        .client
                        .responses_v2()
                        .retained_output(
                            "response",
                            cursor.as_deref(),
                            Some(&cma_sdk::agents::RetainedOutputAvailability { available: true }),
                        )
                        .await
                } else {
                    server
                        .client
                        .agents()
                        .retained_output(
                            "agent",
                            cursor.as_deref(),
                            Some(&cma_sdk::agents::RetainedOutputAvailability { available: true }),
                        )
                        .await
                }
                .unwrap();
                actual.push_str(&page.content);
                cursor = page.next;
                offset = end;
            }
            assert_eq!(actual, text);
            assert_eq!(server.request_count(), requests);
            server.assert_drained();
        }
    }
}
#[tokio::test]
async fn retained_output_refuses_inconsistent_identity_bounds_and_revocation() {
    let server = WireServer::new().await;
    for fault in [
        "foreign",
        "complete",
        "oversize",
        "empty",
        "offset",
        "truncated",
    ] {
        let mut page = json!({"output":output(),"offset":0,"content":"first", "next":"next"});
        match fault {
            "foreign" => page["output"]["agent_id"] = json!("foreign"),
            "complete" => page["output"]["partial"] = json!(false),
            "oversize" => page["content"] = json!("x".repeat(512 * 1024 + 1)),
            "empty" => page["content"] = json!(""),
            "offset" => page["offset"] = json!(1),
            "truncated" => page["next"] = Value::Null,
            _ => unreachable!(),
        }
        expect(&server, "/api/v1/agents/agent/retained-output", page, 200);
        assert!(
            matches!(
                server
                    .client
                    .agents()
                    .retained_output(
                        "agent",
                        None,
                        Some(&cma_sdk::agents::RetainedOutputAvailability { available: true })
                    )
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{fault}"
        );
    }
    expect(
        &server,
        "/api/v1/agents/agent/retained-output?cursor=next",
        json!({"code":"agent_not_found","detail":"No Agent matches this request."}),
        404,
    );
    assert!(matches!(
        server
            .client
            .agents()
            .retained_output(
                "agent",
                Some("next"),
                Some(&cma_sdk::agents::RetainedOutputAvailability { available: true })
            )
            .await,
        Err(Error::Api { status: 404, .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn absent_typed_discovery_never_probes_either_surface() {
    let server = WireServer::new().await;
    for availability in [
        None,
        Some(cma_sdk::agents::RetainedOutputAvailability { available: false }),
    ] {
        assert_eq!(
            server
                .client
                .agents()
                .retained_output("agent", None, availability.as_ref())
                .await
                .unwrap(),
            Default::default()
        );
        assert_eq!(
            server
                .client
                .responses_v2()
                .retained_output("response", None, availability.as_ref())
                .await
                .unwrap(),
            Default::default()
        );
    }
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
}
