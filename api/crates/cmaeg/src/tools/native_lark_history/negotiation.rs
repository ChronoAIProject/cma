use super::*;
use mixed::{enable, post};

fn mixed_row(f: &Fixture, id: &str, offset: i64) -> Value {
    post(
        id,
        f.created - offset,
        f.threaded,
        "Mixed",
        json!([
            {"tag":"text","text":" visible text"},
            {"tag":"img","image_key":"img_never_exposed"}
        ]),
    )
}

#[tokio::test]
async fn native_history_opt_in_defaults_to_legacy_and_pins_every_cursor_page() {
    let mut f = Fixture::new(LarkRegion::Lark, true, true, true).await;
    enable(&mut f);
    f.set_page("", vec![mixed_row(&f, "om_mixed", 1)], Some("next"));
    let legacy = f.read(None).await.unwrap();
    let explicit_false = f.read_with_media(None, false).await.unwrap();
    assert!(legacy.messages.is_empty() && explicit_false.messages.is_empty());
    assert_eq!(legacy.omitted_message_count, 1);
    let legacy_cursor = cursor(legacy);
    let opened = legacy_cursor
        .open_native_lark(&f.app.state.keys, now())
        .unwrap()
        .payload;
    assert_eq!(opened.v, 2);
    assert!(!opened.include_media);
    assert!(serde_json::to_value(&opened)
        .unwrap()
        .get("include_media")
        .is_none());

    let mixed = f.read_with_media(None, true).await.unwrap();
    assert_eq!(
        mixed.messages[0].omitted_content,
        [cmaeg_core::conversation_history::HistoryOmittedContent::Media]
    );
    let mixed_cursor = cursor(mixed);
    let opened = mixed_cursor
        .open_native_lark(&f.app.state.keys, now())
        .unwrap()
        .payload;
    assert_eq!(opened.v, 3);
    assert!(opened.include_media);
    assert_eq!(opened.expires_at, f.app.event.expires_at);
    let calls = f.api.calls.lock().unwrap().len();
    for (cursor, requested) in [(&legacy_cursor, true), (&mixed_cursor, false)] {
        let error = f
            .read_with_media(Some(cursor), requested)
            .await
            .unwrap_err();
        assert_eq!(error.status, 400);
        assert_eq!(
            f.api.calls.lock().unwrap().len(),
            calls,
            "mode mismatch never calls Lark"
        );
    }
    assert_eq!(f.read(Some(&mixed_cursor)).await.unwrap_err().status, 400);
    f.set_page("next", vec![mixed_row(&f, "om_older", 2)], None);
    let old_next = f.read(Some(&legacy_cursor)).await.unwrap();
    assert!(old_next.messages.is_empty());
    let mixed_next = f.read_with_media(Some(&mixed_cursor), true).await.unwrap();
    assert_eq!(mixed_next.messages[0].text, "Mixed visible text");
    assert!(mixed_next.messages[0].attachments.is_empty());
    assert_eq!(mixed_next.expires_at, f.app.event.expires_at);
    assert!(!serde_json::to_string(&mixed_next)
        .unwrap()
        .contains("img_never_exposed"));
}

#[tokio::test]
async fn native_history_opt_in_requires_both_gates_and_original_source_consent() {
    let mut f = Fixture::new(LarkRegion::Feishu, false, false, true).await;
    f.set_page("", vec![mixed_row(&f, "om_mixed", 1)], Some("next"));
    assert_eq!(f.read_with_media(None, true).await.unwrap_err().status, 403);
    assert_eq!(f.list_calls(), 0);
    enable(&mut f);
    let mixed_cursor = cursor(f.read_with_media(None, true).await.unwrap());
    Arc::get_mut(
        f.app
            .state
            .native_lark_conversation_history
            .as_mut()
            .unwrap(),
    )
    .unwrap()
    .mixed_media = false;
    assert_eq!(
        f.read_with_media(Some(&mixed_cursor), true)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(f.list_calls(), 1);
    assert!(f.read(None).await.unwrap().messages.is_empty());

    let mut no_consent = Fixture::new(LarkRegion::Feishu, false, false, false).await;
    enable(&mut no_consent);
    no_consent.set_page("", vec![mixed_row(&no_consent, "om_mixed", 1)], None);
    assert!(no_consent.read_with_media(None, true).await.is_err());
    assert_eq!(no_consent.list_calls(), 0);
    enable(&mut f);
    let Source::NativeLark(source) = &mut f.app.definition.sources[0] else {
        panic!()
    };
    source.conversation_history = false;
    f.app.replace();
    let calls = f.list_calls();
    assert_eq!(
        f.read_with_media(Some(&mixed_cursor), true)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(f.list_calls(), calls);
    f.app.state.native_lark_conversation_history = None;
    assert_eq!(f.read_with_media(None, true).await.unwrap_err().status, 403);
}

#[tokio::test]
async fn native_history_cursor_version_and_negotiation_cannot_disagree() {
    let mut f = Fixture::new(LarkRegion::Lark, false, false, true).await;
    enable(&mut f);
    f.set_page("", vec![mixed_row(&f, "om_mixed", 1)], Some("next"));
    for include_media in [false, true] {
        let issued = cursor(f.read_with_media(None, include_media).await.unwrap());
        let mut payload = issued
            .open_native_lark(&f.app.state.keys, now())
            .unwrap()
            .payload;
        payload.include_media = !include_media;
        assert!(!payload.valid());
        assert!(HistoryCursor::seal_native_lark(&payload, &f.app.state.keys).is_err());
    }
}

#[tokio::test]
async fn native_history_rest_and_mcp_negotiate_explicitly_without_changing_old_clients() {
    let mut f = Fixture::new(LarkRegion::Lark, true, true, true).await;
    enable(&mut f);
    f.set_page("", vec![mixed_row(&f, "om_mixed", 1)], None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::http::tools::router(f.app.state.clone())
        .merge(crate::mcp::router(f.app.state.clone()));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::new();
    let path = format!("{base}/v1/events/{}/history", f.reference().as_str());
    for (query, count) in [
        ("", 0),
        ("?include_media=false", 0),
        ("?include_media=true", 1),
    ] {
        let response = http
            .get(format!("{path}{query}"))
            .bearer_auth(&f.app.key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let page: Value = response.json().await.unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), count);
        if count == 0 {
            assert!(!page.to_string().contains("\"media\""));
        }
    }
    for query in [
        "?include_media=maybe",
        "?include_media=true&include_media=false",
    ] {
        assert_eq!(
            http.get(format!("{path}{query}"))
                .bearer_auth(&f.app.key)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let catalog: Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&f.app.key)
        .json(&json!({"jsonrpc":"2.0","id":0,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let tool = catalog["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "readEventHistory")
        .unwrap();
    assert_eq!(
        tool["inputSchema"]["properties"]["include_media"]["type"],
        "boolean"
    );
    assert!(!tool["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "include_media"));
    for include in [None, Some(false), Some(true)] {
        let mut args = json!({"event_ref":f.reference()});
        if let Some(value) = include {
            args["include_media"] = value.into();
        }
        let response: Value = http.post(format!("{base}/mcp")).bearer_auth(&f.app.key)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"readEventHistory","arguments":args}}))
            .send().await.unwrap().json().await.unwrap();
        assert!(response.get("error").is_none(), "{response}");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        let page: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            page["messages"].as_array().unwrap().len(),
            usize::from(include == Some(true)),
            "{page}"
        );
    }
    assert_eq!(f.list_calls(), 6);
    server.abort();
}
