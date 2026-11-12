//! Browser navigation evidence bridges an external click across sign-in.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use layer2_core::trigger_domain::{PlacementId, PublicTriggerId, TriggerAvailability};
use serde::Serialize;
use utoipa::ToSchema;

use super::{is_embed_identifier, TriggerAssetsState};

const MAX_AGE: u64 = 600;

#[derive(Serialize, ToSchema)]
pub struct TriggerEntryIntentView {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/triggers/public/{public_id}/open/{placement_id}", tag = "triggers", params(("public_id" = String, Path), ("placement_id" = String, Path)), responses((status = 303, description = "Return to the public Trigger page without starting a Launch"), (status = 404)), security(()))]
pub(super) async fn open(
    State(state): State<TriggerAssetsState>,
    Path((public_id, placement_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !valid_ids(&public_id, &placement_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let trigger = match state
        .triggers
        .trigger_by_public_id(&PublicTriggerId(public_id.clone()))
        .await
    {
        Ok(Some(trigger)) if trigger.availability == TriggerAvailability::Published => trigger,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    match state.standing.erasing(&trigger.owner).await {
        Ok(false) => {}
        _ => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    match state
        .triggers
        .placement(
            &trigger.trigger_id,
            &trigger.owner,
            &PlacementId(placement_id.clone()),
        )
        .await
    {
        Ok(Some(placement)) if placement.retired_at.is_none() => {}
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let mut response = Redirect::to(&format!("/t/{public_id}/{placement_id}")).into_response();
    let deliberate = header_is(&headers, "sec-fetch-user", "?1")
        && header_is(&headers, "sec-fetch-mode", "navigate")
        && header_is(&headers, "sec-fetch-dest", "document")
        && !headers.contains_key("purpose")
        && !headers.contains_key("sec-purpose");
    let value = if deliberate && !trigger.revision.content.consent.always_review {
        format!("{}.{placement_id}", now_seconds())
    } else {
        String::new()
    };
    set_cookie(&state, &mut response, &public_id, &value);
    private_headers(&mut response);
    response
}

#[utoipa::path(post, path = "/api/v1/triggers/public/{public_id}/entry-intent", tag = "triggers", params(("public_id" = String, Path)), responses((status = 200, body = TriggerEntryIntentView), (status = 403)), security(()))]
pub(super) async fn consume(
    State(state): State<TriggerAssetsState>,
    Path(public_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !public_id.starts_with("pub_") || !is_embed_identifier(&public_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let origin = state.public_origin().origin().ascii_serialization();
    if !header_is(&headers, "origin", &origin)
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value != "same-origin")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    // This timestamp/placement cookie records navigation only. Publisher
    // authority is checked by open and by launch admission, never by consuming it.
    let name = cookie_name(&state, &public_id);
    let placement_id = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| value.split_once('.'))
        .and_then(|(timestamp, placement)| {
            let age = now_seconds().checked_sub(timestamp.parse::<u64>().ok()?)?;
            (age <= MAX_AGE && valid_ids(&public_id, placement)).then(|| placement.to_string())
        });
    let mut response = Json(TriggerEntryIntentView {
        accepted: placement_id.is_some(),
        placement_id,
    })
    .into_response();
    set_cookie(&state, &mut response, &public_id, "");
    private_headers(&mut response);
    response
}

fn valid_ids(public_id: &str, placement_id: &str) -> bool {
    public_id.starts_with("pub_")
        && placement_id.starts_with("plc_")
        && is_embed_identifier(public_id)
        && is_embed_identifier(placement_id)
}

fn header_is(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    let mut values = headers.get_all(name).iter();
    values.next().is_some_and(|value| value == expected) && values.next().is_none()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cookie_name(state: &TriggerAssetsState, public_id: &str) -> String {
    let prefix = if state.public_origin().scheme() == "https" {
        "__Host-"
    } else {
        ""
    };
    format!("{prefix}cma_entry_{public_id}")
}

fn set_cookie(state: &TriggerAssetsState, response: &mut Response, public_id: &str, value: &str) {
    let secure = if state.public_origin().scheme() == "https" {
        "; Secure"
    } else {
        ""
    };
    let age = if value.is_empty() { 0 } else { MAX_AGE };
    let cookie = format!(
        "{}={value}; Path=/; Max-Age={age}; HttpOnly; SameSite=Strict{secure}",
        cookie_name(state, public_id)
    );
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("validated cookie fields"),
    );
}

fn private_headers(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::rest::trigger_assets_test_support::{Fixture, ORIGIN, PLACEMENT_ID, PUBLIC_ID};

    #[tokio::test]
    async fn only_deliberate_document_navigation_carries_no_review_intent() {
        let fixture = Fixture::no_review();
        let uri = format!("/api/v1/triggers/public/{PUBLIC_ID}/open/{PLACEMENT_ID}");
        for deliberate in [false, true] {
            let mut request = Request::get(&uri);
            if deliberate {
                request = request
                    .header("sec-fetch-user", "?1")
                    .header("sec-fetch-mode", "navigate")
                    .header("sec-fetch-dest", "document");
            }
            let response = fixture
                .router()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(
                response.headers()[header::LOCATION],
                format!("/t/{PUBLIC_ID}/{PLACEMENT_ID}")
            );
            let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
            assert_eq!(cookie.contains("Max-Age=600"), deliberate);
            let intent = fixture
                .router()
                .oneshot(
                    Request::post(format!("/api/v1/triggers/public/{PUBLIC_ID}/entry-intent"))
                        .header(header::ORIGIN, ORIGIN)
                        .header("sec-fetch-site", "same-origin")
                        .header(header::COOKIE, cookie.split(';').next().unwrap())
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(intent.status(), StatusCode::OK);
            assert!(intent.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0"));
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(intent.into_body(), 2048).await.unwrap()).unwrap();
            assert_eq!(body["accepted"], deliberate);
        }
    }

    #[tokio::test]
    async fn review_scanners_and_cross_origin_consumption_do_not_authorize_start() {
        for (fixture, purpose) in [
            (Fixture::published(), None),
            (Fixture::no_review(), Some("prefetch")),
        ] {
            let mut request = Request::get(format!(
                "/api/v1/triggers/public/{PUBLIC_ID}/open/{PLACEMENT_ID}"
            ))
            .header("sec-fetch-user", "?1")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document");
            if let Some(purpose) = purpose {
                request = request.header("sec-purpose", purpose);
            }
            let response = fixture
                .router()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(response.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("Max-Age=0"));
        }
        let response = Fixture::no_review()
            .router()
            .oneshot(
                Request::post(format!("/api/v1/triggers/public/{PUBLIC_ID}/entry-intent"))
                    .header(header::ORIGIN, "https://elsewhere.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    #[tokio::test]
    async fn consuming_navigation_intent_preserves_cookie_headers_without_public_lookup() {
        let fixture = Fixture::no_review();
        let cookie = format!(
            "__Host-cma_entry_{PUBLIC_ID}={}.{PLACEMENT_ID}",
            now_seconds()
        );
        let response = fixture
            .router()
            .oneshot(
                Request::post(format!("/api/v1/triggers/public/{PUBLIC_ID}/entry-intent"))
                    .header(header::ORIGIN, ORIGIN)
                    .header("sec-fetch-site", "same-origin")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert!(response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0"));
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 2048).await.unwrap()).unwrap();
        assert_eq!(body["accepted"], true);
        assert_eq!(body["placement_id"], PLACEMENT_ID);
        assert_eq!(fixture.store.public_lookups(), 0);
    }
    #[tokio::test]
    async fn opening_navigation_uses_existing_publisher_snapshot_without_extra_lookup() {
        use crate::rest::shared_stream_tests::ChangingStanding;
        for deleting in [None, Some("publisher"), Some("outage")] {
            let fixture = Fixture::no_review();
            let standing = std::sync::Arc::new(ChangingStanding::default());
            *standing.0.lock().unwrap() = deleting;
            let state = super::super::TriggerAssetsState::new(fixture.store.clone(), ORIGIN)
                .unwrap()
                .with_account_standing(standing);
            let response = super::super::trigger_assets_router(state)
                .oneshot(
                    Request::get(format!(
                        "/api/v1/triggers/public/{PUBLIC_ID}/open/{PLACEMENT_ID}"
                    ))
                    .header("sec-fetch-user", "?1")
                    .header("sec-fetch-mode", "navigate")
                    .header("sec-fetch-dest", "document")
                    .body(Body::empty())
                    .unwrap(),
                )
                .await
                .unwrap();
            if deleting.is_none() {
                assert_eq!(response.status(), StatusCode::SEE_OTHER);
                assert_eq!(
                    response.headers()[header::CACHE_CONTROL],
                    "private, no-store"
                );
                assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
                assert!(response.headers()[header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .contains("Max-Age=600"));
            } else {
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            }
            assert_eq!(fixture.store.public_lookups(), 1);
        }
    }
}
