use super::*;

#[tokio::test]
async fn title_proxy_accepts_only_unambiguous_utf8_json() {
    for (content_types, accepted) in [
        (&["application/json"][..], true),
        (&["application/json; charset=utf-8"], true),
        (&["Application/JSON; Charset=\"UTF-8\""], true),
        (&[], false),
        (&["text/plain; charset=utf-8"], false),
        (&["application/jsonl; charset=utf-8"], false),
        (&["application/json; charset=iso-8859-1"], false),
        (&["application/json; charset=utf-8; charset=utf-8"], false),
        (&["application/json, application/json"], false),
        (
            &["application/json", "application/json; charset=utf-8"],
            false,
        ),
    ] {
        let route = post(move || async move {
            let mut response = Json(serde_json::json!({
                "status": "completed",
                "output": [{"type": "message", "content": [
                    {"type": "output_text", "text": "Orchard Budget Review"}
                ]}]
            }))
            .into_response();
            response.headers_mut().remove(header::CONTENT_TYPE);
            for content_type in content_types {
                response
                    .headers_mut()
                    .append(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            }
            response
        });
        let service = "/api/v1/proxy/s/chrono-llm-public";
        let (origin, server) =
            spawn(Router::new().route(&format!("{service}/responses"), route)).await;
        let generator = HttpAgentTitleGenerator::fixture(
            &format!("{origin}{service}"),
            DEPLOYMENT_MODEL,
            Duration::from_secs(2),
            MAX_RESPONSE_BYTES,
        );
        let (outcome, stored) =
            generate_title(&generator, DEPLOYMENT_MODEL, BEARER, "Review costs", "Done").await;
        server.abort();
        if accepted {
            assert_eq!(
                outcome,
                Ok(AgentTitleOutcome::Generated),
                "{content_types:?}"
            );
            let title = stored.expect("the provider title is persisted");
            assert_eq!(title.source, AgentTitleSource::Generated);
            assert_eq!(title.title.as_str(), "Orchard Budget Review");
        } else {
            assert_eq!(
                outcome,
                Err(AgentTitleServiceError::GenerationUnavailable),
                "{content_types:?}"
            );
            assert!(stored.is_none());
        }
    }
}
