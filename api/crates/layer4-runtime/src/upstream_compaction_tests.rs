use super::*;

fn fixture() -> Vec<u8> {
    // The unusually large number and escaped strings must survive byte for
    // byte, including values that serde_json::Value would round through f64.
    br#"{
  "model": "gpt-6",
  "input": [{"type":"web_search_call","id":"search-1","action":{"type":"search","query":"fixture"}},
            {"type":"reasoning","encrypted_content":"abc\\def\u002fghi","unrecognized":1844674407370955161600}],
  "tools" : [  ],
  "tool_choice" : "auto",
  "reasoning": {"effort":"high"},
  "client_metadata": {"x-codex-turn-metadata":"{\"request_kind\":\"compaction\",\"compaction\":{\"implementation\":\"responses\",\"reason\":\"context_limit\",\"strategy\":\"memento\"}}"},
  "future_extension": {"nested_tools": [], "tool_choice":"auto"}
}"#.to_vec()
}

fn headers() -> HeaderMap {
    HeaderMap::from_iter([(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    )])
}

fn normalize_fixture(body: &[u8]) -> Option<Vec<u8>> {
    normalize(
        &Method::POST,
        &"/responses".parse().unwrap(),
        &headers(),
        body,
    )
}

#[test]
fn compaction_preserves_history_bytes_and_forbids_tool_execution() {
    let original = fixture();
    let normalized = normalize_fixture(&original).unwrap();
    let expected = String::from_utf8(original)
        .unwrap()
        .replace(
            "\"tools\" : [  ]",
            "\"tools\" : [{\"type\":\"web_search\",\"external_web_access\":false}]",
        )
        .replace("\"tool_choice\" : \"auto\"", "\"tool_choice\" : \"none\"");
    assert_eq!(normalized, expected.as_bytes());
    assert!(
        normalize_fixture(&normalized).is_none(),
        "already declared tools stay unchanged"
    );
}

#[test]
fn handles_reverse_field_order_without_corrupting_spans() {
    let original = String::from_utf8(fixture()).unwrap().replace(
        "\"tools\" : [  ],\n  \"tool_choice\" : \"auto\"",
        "\"tool_choice\" : \"none\",\n  \"tools\" : [  ]",
    );
    let expected = original.replace("[  ]", std::str::from_utf8(SEARCH_DECLARATION).unwrap());
    assert_eq!(
        normalize_fixture(original.as_bytes()).unwrap(),
        expected.as_bytes()
    );
}

#[test]
fn ordinary_turns_and_unrecognized_request_shapes_remain_unchanged() {
    let original = String::from_utf8(fixture()).unwrap();
    for (before, after) in [
        ("\\\"compaction\\\"", "\\\"turn\\\""),
        ("\\\"responses\\\"", "\\\"remote\\\""),
        ("web_search_call", "function_call"),
        (
            "\"tools\" : [  ]",
            "\"tools\" : [{\"type\":\"function\",\"name\":\"keep\"}]",
        ),
        ("\"tools\" : [  ]", "\"tools\" : null"),
        ("\"tools\" : [  ]", "\"unknown_tools\" : [  ]"),
        (
            "\"tool_choice\" : \"auto\"",
            "\"tool_choice\" : \"required\"",
        ),
        ("\"tool_choice\" : \"auto\"", "\"tool_choice\" : null"),
        (
            "\"tool_choice\" : \"auto\"",
            "\"tool_choice\" : {\"type\":\"function\",\"name\":\"keep\"}",
        ),
        (
            "\"tool_choice\" : \"auto\"",
            "\"unknown_choice\" : \"auto\"",
        ),
        (
            "\"effort\":\"high\"",
            "\"effort\":\"high\",\"context\":\"all_turns\"",
        ),
        ("\"type\":\"reasoning\"", "\"type\":\"additional_tools\""),
        ("x-codex-turn-metadata", "unknown-metadata"),
        ("\"tools\" : [  ]", "\"tools\":[], \"tools\":[]"),
    ] {
        let changed = original.replace(before, after);
        assert_ne!(changed, original, "fixture mutation should apply: {before}");
        assert!(
            normalize_fixture(changed.as_bytes()).is_none(),
            "should keep original shape: {after}"
        );
    }
    for body in [b"{".as_slice(), b"null", b"[]"] {
        assert!(normalize_fixture(body).is_none());
    }
}

#[test]
fn normalization_requires_the_exact_json_responses_route() {
    let body = fixture();
    for route in ["/responses", "/v1/responses"] {
        assert!(normalize(&Method::POST, &route.parse().unwrap(), &headers(), &body).is_some());
    }
    for (method, route) in [
        (Method::GET, "/responses"),
        (Method::POST, "/responses/compact"),
        (Method::POST, "/api/v1/proxy/s/chrono-llm-public/responses"),
        (Method::POST, "/images/generations"),
    ] {
        assert!(normalize(&method, &route.parse().unwrap(), &headers(), &body).is_none());
    }
    for (name, value) in [
        ("content-type", "text/plain"),
        ("content-encoding", "gzip"),
        ("x-openai-internal-codex-responses-lite", "true"),
    ] {
        let mut headers = headers();
        headers.insert(
            axum::http::HeaderName::from_static(name),
            value.parse().unwrap(),
        );
        assert!(normalize(
            &Method::POST,
            &"/responses".parse().unwrap(),
            &headers,
            &body
        )
        .is_none());
    }
}
