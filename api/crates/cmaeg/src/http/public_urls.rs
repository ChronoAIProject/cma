//! Canonical navigation into the configured console OAuth authority.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use reqwest::Url;

pub(crate) fn router(state: &super::ManagementState) -> Router {
    let api = Url::parse(&state.public_api_url).expect("validated public API base");
    let prefix = api.path().trim_end_matches('/');
    Router::new()
        .route(&format!("{prefix}/legacy-console"), get(index))
        .route(
            &format!("{prefix}/legacy-console/channels/{{record}}"),
            get(channel),
        )
        .with_state(state.console_public_url.clone())
}
async fn index(State(origin): State<String>) -> Response {
    Redirect::temporary(&format!("{origin}/console")).into_response()
}
async fn channel(State(origin): State<String>, Path(record): Path<String>) -> Response {
    if record.is_empty()
        || record.len() > 512
        || matches!(record.as_str(), "." | "..")
        || record
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(mut url) = Url::parse(&origin) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(mut path) = url.path_segments_mut() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    path.pop_if_empty().extend(["console", "channels", &record]);
    drop(path);
    Redirect::temporary(url.as_str()).into_response()
}
