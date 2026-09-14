#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    agents::TranscriptTurn,
    assets::{EmbedScriptOptions, EmbedScriptResponse},
    triggers::BadgeOptions,
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};
use wire::{Expected, WireServer};

#[tokio::test]
async fn embed_asset_is_exact_anonymous_javascript_with_explicit_conditional_semantics() {
    let server = WireServer::new().await;
    let bytes = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/public/embed/v1.js"),
    )
    .unwrap();
    server.expect(Expected {
        method: "GET".into(),
        path: "/embed/v1.js".into(),
        headers: wire::headers(false, "application/javascript"),
        status: 200,
        response_headers: BTreeMap::from([
            (
                "content-type".into(),
                "application/javascript; charset=utf-8".into(),
            ),
            ("etag".into(), "\"script-v1\"".into()),
        ]),
        response: bytes.clone(),
        ..Default::default()
    });
    let result = server
        .client
        .read_embed_script(&EmbedScriptOptions::default())
        .await
        .unwrap();
    assert_eq!(
        result,
        EmbedScriptResponse::Modified {
            script: String::from_utf8(bytes).unwrap(),
            etag: Some("\"script-v1\"".into())
        }
    );
    let mut headers = wire::headers(false, "application/javascript");
    headers.insert("if-none-match".into(), "\"script-v1\"".into());
    server.expect(Expected {
        method: "GET".into(),
        path: "/embed/v1.js".into(),
        headers,
        status: 304,
        response_headers: BTreeMap::from([("etag".into(), "\"script-v1\"".into())]),
        ..Default::default()
    });
    assert_eq!(
        server
            .client
            .read_embed_script(&EmbedScriptOptions {
                if_none_match: Some("\"script-v1\"".into())
            })
            .await
            .unwrap(),
        EmbedScriptResponse::NotModified {
            etag: Some("\"script-v1\"".into())
        }
    );
    assert_eq!(server.request_count(), 2);
    let _ = (&server.anonymous, &server.base_url);
    server.assert_drained();
}

#[tokio::test]
async fn embed_asset_refuses_html_and_invalid_headers_without_following_redirects() {
    let server = WireServer::new().await;
    assert!(matches!(
        server
            .client
            .read_embed_script(&EmbedScriptOptions {
                if_none_match: Some("invalid\r\nheader".into())
            })
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
    for (status, content_type) in [(200, "text/html"), (302, "text/plain")] {
        server.expect(Expected {
            method: "GET".into(),
            path: "/embed/v1.js".into(),
            headers: wire::headers(false, "application/javascript"),
            status,
            response_headers: BTreeMap::from([
                ("content-type".into(), content_type.into()),
                ("location".into(), "https://foreign.example/script".into()),
            ]),
            response: b"not JavaScript".to_vec(),
            ..Default::default()
        });
        assert!(matches!(
            server
                .client
                .read_embed_script(&EmbedScriptOptions::default())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn asset_cache_hits_require_a_request_validator_and_badges_require_an_etag() {
    let server = WireServer::new().await;
    for badge in [false, true] {
        server.expect(Expected {
            method: "GET".into(),
            path: if badge {
                "/api/v1/triggers/public/public_contract/badge.svg"
            } else {
                "/embed/v1.js"
            }
            .into(),
            headers: wire::headers(
                false,
                if badge {
                    "image/svg+xml"
                } else {
                    "application/javascript"
                },
            ),
            status: 304,
            response_headers: BTreeMap::from([("etag".into(), "\"v1\"".into())]),
            ..Default::default()
        });
        let error = if badge {
            server
                .client
                .read_trigger_badge("public_contract", &BadgeOptions::default())
                .await
                .unwrap_err()
        } else {
            server
                .client
                .read_embed_script(&EmbedScriptOptions::default())
                .await
                .unwrap_err()
        };
        assert!(
            matches!(error, Error::Protocol { message } if message.contains("unsolicited 304"))
        );
    }
    for status in [200, 304] {
        for etag in [None, Some("")] {
            let mut response_headers =
                BTreeMap::from([("content-type".into(), "image/svg+xml".into())]);
            if let Some(etag) = etag {
                response_headers.insert("etag".into(), etag.into());
            }
            let mut headers = wire::headers(false, "image/svg+xml");
            headers.insert("if-none-match".into(), "\"v1\"".into());
            server.expect(Expected {
                method: "GET".into(),
                path: "/api/v1/triggers/public/public_contract/badge.svg".into(),
                headers,
                status,
                response_headers,
                response: if status == 200 {
                    b"<svg></svg>".to_vec()
                } else {
                    vec![]
                },
                ..Default::default()
            });
            assert!(matches!(
                server.client.read_trigger_badge("public_contract", &BadgeOptions {
                    if_none_match: Some("\"v1\"".into()),
                    ..Default::default()
                }).await,
                Err(Error::Protocol { message }) if message.contains("entity tag")
            ));
        }
    }
    assert_eq!(server.request_count(), 6);
    server.assert_drained();
}

#[test]
fn chronological_transcript_messages_are_required_and_closed() {
    let valid = json!({"turn_sequence":1,"started_at":"2026-09-07T00:00:00Z","owner_message":"Initial","agent_messages":["Answer"],
        "messages":[{"sequence":2,"role":"user","content":"Initial","appended_at":"2026-09-07T00:00:00Z"},
        {"sequence":3,"role":"assistant","content":"Answer","appended_at":"2026-09-07T00:00:00Z"},
        {"sequence":4,"role":"user","content":"Steer","appended_at":"2026-09-07T00:00:01Z"}],"outcome":{"type":"active"}});
    let turn: TranscriptTurn = serde_json::from_value(valid.clone()).unwrap();
    assert_eq!(
        turn.messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["Initial", "Answer", "Steer"]
    );
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("messages");
    assert!(serde_json::from_value::<TranscriptTurn>(missing).is_err());
    for bad in [json!(0), json!("unknown")] {
        let mut value = valid.clone();
        if bad.is_number() {
            value["messages"][0]["sequence"] = bad;
        } else {
            value["messages"][0]["role"] = bad;
        }
        assert!(serde_json::from_value::<TranscriptTurn>(value).is_err());
    }
}
