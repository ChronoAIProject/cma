//! Advisory live-session allowance for the authenticated caller.

use std::sync::Arc;

use axum::{extract::State, Json};
use layer2_core::{SessionAllowance, SessionService};
use serde::Serialize;

use crate::auth::AuthenticatedCaller;
use crate::rest::Problem;

#[derive(Debug, Serialize)]
pub(super) struct AllowanceView {
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    live: u32,
    exempt: bool,
}

impl From<SessionAllowance> for AllowanceView {
    fn from(value: SessionAllowance) -> Self {
        Self {
            limit: value.limit,
            live: value.live,
            exempt: value.exempt,
        }
    }
}

pub(super) async fn read(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Result<Json<AllowanceView>, Problem> {
    Ok(Json(service.session_allowance(&caller).await?.into()))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        extract::Path,
        http::Request,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn an_absent_limit_is_omitted_rather_than_rendered_null() {
        let value = serde_json::to_value(AllowanceView::from(SessionAllowance {
            limit: None,
            live: 2,
            exempt: false,
        }))
        .unwrap();
        assert!(value.get("limit").is_none());
        assert_eq!(value["live"], 2);
        assert_eq!(value["exempt"], false);
    }

    #[tokio::test]
    async fn allowance_and_identifier_paths_both_resolve_to_their_own_route() {
        async fn allowance() -> &'static str {
            "allowance"
        }
        async fn session(Path(id): Path<String>) -> String {
            format!("session:{id}")
        }
        let app = Router::new()
            .route("/api/v1/sessions/allowance", get(allowance))
            .route("/api/v1/sessions/{id}", get(session));

        for (path, expected) in [
            ("/api/v1/sessions/allowance", "allowance"),
            ("/api/v1/sessions/sesn_1", "session:sesn_1"),
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body.as_ref(), expected.as_bytes());
        }
    }
}
