//! Bounded channel schema diagnostics; never echo submitted fields or values.
use axum::{extract::rejection::JsonRejection, http::StatusCode, response::Response};
use cmaeg_core::problem::ProblemCode;
use std::error::Error;

const ROUTING_DETAIL: &str = "/routing: not accepted in channel create or full update; use PUT /v1/channels/{channel_id}/routing with {expected_version,routing} after creation";

pub(super) fn rejection(error: &JsonRejection) -> Response {
    let misplaced_routing = is_misplaced_routing(error);
    let reason = if misplaced_routing {
        "routing_requires_policy_endpoint"
    } else {
        "channel_schema_mismatch"
    };
    // The existing request span supplies the route template. Neither the serde error
    // nor the request body is safe to log: both can include credentials/content.
    tracing::info!(reason, "channel request schema rejected");
    super::problems::no_store(super::problems::problem(
        StatusCode::BAD_REQUEST,
        ProblemCode::InvalidRequest,
        if misplaced_routing {
            ROUTING_DETAIL
        } else {
            "request does not match the channel schema"
        },
    ))
}

fn is_misplaced_routing(error: &JsonRejection) -> bool {
    let mut source: Option<&(dyn Error + 'static)> = Some(error);
    while let Some(error) = source {
        if let Some(error) = error.downcast_ref::<serde_path_to_error::Error<serde_json::Error>>() {
            let mut path = error.path().iter();
            return matches!(path.next(), Some(serde_path_to_error::Segment::Map { key }) if key == "routing")
                && path.next().is_none()
                && error
                    .inner()
                    .to_string()
                    .starts_with("unknown field `routing`,");
        }
        source = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::definition::ChannelCreateRequest;
    use axum::Json;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn schema_diagnostics_log_only_fixed_reasons_without_request_data() {
        // Other tests may have reached this callsite without a subscriber.
        let _shared = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let capture = Capture::default();
        let writer = capture.clone();
        let _logs = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish(),
        );
        for (input, detail) in [
            (
                r#"{"credentials":{"agent_key":"private-worker-key","discord_bot_token":"private-bot-token"},"routing":"private-value"}"#,
                ROUTING_DETAIL,
            ),
            (
                r#"{"credentials":{"agent_key":"private-worker-key","routing":"private-value"}}"#,
                "request does not match the channel schema",
            ),
            (
                r#"{"private-field-name":"private-value"}"#,
                "request does not match the channel schema",
            ),
        ] {
            let error = Json::<ChannelCreateRequest>::from_bytes(input.as_bytes()).unwrap_err();
            let body = axum::body::to_bytes(rejection(&error).into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["detail"], detail);
        }
        let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("routing_requires_policy_endpoint"));
        assert!(logs.contains("channel_schema_mismatch"));
        assert!(!logs.contains("private-"));
        assert!(!logs.contains("unknown field"));
    }
}
