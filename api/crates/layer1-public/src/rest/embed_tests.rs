use std::path::PathBuf;

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
    response::Response,
    Router,
};
use layer2_core::{
    trigger_domain::{ButtonLabel, Theme},
    SubjectId,
};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    embed::{build_descriptor, EmbedDescriptor, EMBED_V1_JAVASCRIPT_SRI},
    trigger_assets_test_support::{
        Fixture, ORIGIN, OTHER_OWNER, OWNER, OWNER_SECRET, PLACEMENT_ID, PUBLIC_ID, TRIGGER_ID,
    },
};
use crate::auth::AuthenticatedSubject;

fn embed_path(trigger_id: &str, placement_id: &str) -> String {
    format!("/api/v1/triggers/{trigger_id}/embed?placement_id={placement_id}")
}

fn request(path: impl AsRef<str>, subject: Option<&str>) -> Request<Body> {
    let mut request = Request::get(path.as_ref()).body(Body::empty()).unwrap();
    if let Some(subject) = subject {
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId(subject.into())));
    }
    request
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 16 * 1_024)
            .await
            .expect("descriptor body is bounded")
            .to_vec(),
    )
    .expect("descriptor body is UTF-8")
}

fn descriptor(fixture: &Fixture) -> EmbedDescriptor {
    build_descriptor(
        &url::Url::parse(ORIGIN).unwrap(),
        fixture.trigger.public_id.as_ref().unwrap(),
        fixture.trigger.latest_revision().unwrap(),
        &fixture.placement.id,
    )
}

#[test]
fn every_snippet_and_the_script_integrity_have_one_exact_golden() {
    let fixture = Fixture::published();
    let actual = format!(
        "{}\n",
        serde_json::to_string_pretty(&descriptor(&fixture)).unwrap()
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/trigger-embed/descriptor.json");
    if std::env::var_os("UPDATE_EMBED_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("golden directory is writable");
        std::fs::write(&path, &actual).expect("descriptor golden is writable");
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(actual, expected);
}

#[test]
fn canonical_snippets_match_the_javascript_dom_and_preserve_url_semantics() {
    let fixture = Fixture::published();
    let descriptor = descriptor(&fixture);
    let public_url = format!("{ORIGIN}/t/{PUBLIC_ID}/{PLACEMENT_ID}");
    let badge_url = format!(
        "{ORIGIN}/api/v1/triggers/public/{PUBLIC_ID}/badge.svg?label=open_in_cma&theme=auto&placement={PLACEMENT_ID}"
    );
    assert_eq!(descriptor.public_url, public_url);
    assert_eq!(descriptor.badge_url, badge_url);
    assert_eq!(descriptor.label_text, "Open in CMA");
    assert_eq!(descriptor.javascript_sri, EMBED_V1_JAVASCRIPT_SRI);
    assert_eq!(
        descriptor.snippets.markdown,
        format!("[![Open in CMA]({badge_url})]({public_url})")
    );
    assert_eq!(
        descriptor.snippets.html,
        format!(
            "<a href=\"{public_url}\"><img src=\"{}\" alt=\"Open in CMA\"></a>",
            badge_url.replace('&', "&amp;")
        )
    );
    assert_eq!(
        descriptor.snippets.javascript,
        format!(
            "<script src=\"{ORIGIN}/embed/v1.js\" integrity=\"{EMBED_V1_JAVASCRIPT_SRI}\" crossorigin=\"anonymous\"></script>\n<cma-trigger trigger=\"{PUBLIC_ID}\" placement=\"{PLACEMENT_ID}\" label=\"open_in_cma\" theme=\"auto\" origin=\"{ORIGIN}\"></cma-trigger>"
        )
    );
    assert_eq!(
        descriptor.snippets.react,
        format!("<a href=\"{public_url}\"><img src=\"{badge_url}\" alt=\"Open in CMA\" /></a>")
    );
    assert_eq!(descriptor.snippets.url, public_url);
    assert_eq!(
        url::Url::parse(&descriptor.badge_url)
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>(),
        vec![
            ("label".to_string(), "open_in_cma".to_string()),
            ("theme".to_string(), "auto".to_string()),
            ("placement".to_string(), PLACEMENT_ID.to_string()),
        ]
    );
    assert!(!serde_json::to_string(&descriptor)
        .unwrap()
        .contains(OWNER_SECRET));
}

#[test]
fn every_closed_label_and_theme_maps_to_the_embed_vocabulary() {
    let fixture = Fixture::published();
    let base = fixture.trigger.latest_revision().unwrap();
    for (label, key, text) in [
        (ButtonLabel::OpenInCma, "open_in_cma", "Open in CMA"),
        (
            ButtonLabel::ChatWithThis,
            "chat_with_this",
            "Chat with this",
        ),
        (ButtonLabel::RunWithCma, "run_with_cma", "Run with CMA"),
        (ButtonLabel::TryIt, "try_it", "Try it"),
    ] {
        for (theme, theme_key) in [
            (Theme::Auto, "auto"),
            (Theme::Light, "light"),
            (Theme::Dark, "dark"),
        ] {
            let mut revision = base.clone();
            revision.content.presentation.button_label = label;
            revision.content.presentation.theme = theme;
            let descriptor = build_descriptor(
                &url::Url::parse(ORIGIN).unwrap(),
                fixture.trigger.public_id.as_ref().unwrap(),
                &revision,
                &fixture.placement.id,
            );
            assert_eq!(descriptor.label_text, text);
            assert!(descriptor.badge_url.contains(&format!("label={key}")));
            assert!(descriptor.badge_url.contains(&format!("theme={theme_key}")));
            assert!(descriptor
                .snippets
                .javascript
                .contains(&format!("label=\"{key}\" theme=\"{theme_key}\"")));
        }
    }
}

async fn json_response(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let value = serde_json::from_str(&body(response).await).unwrap();
    (status, headers, value)
}

#[tokio::test]
async fn owner_route_is_authenticated_private_and_exactly_matches_the_generator() {
    let fixture = Fixture::published();
    let app = fixture.router();
    let path = embed_path(TRIGGER_ID, PLACEMENT_ID);

    let unauthenticated = app.clone().oneshot(request(&path, None)).await.unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_private_headers(&unauthenticated);

    let response = app.oneshot(request(path, Some(OWNER))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_private_headers(&response);
    let actual: EmbedDescriptor = serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(actual, descriptor(&fixture));
}

fn assert_private_headers(response: &Response) {
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn wrong_owner_and_missing_ids_share_the_same_not_found_projection() {
    let fixture = Fixture::published();
    let app = fixture.router();
    let wrong_owner = json_response(
        &app,
        request(embed_path(TRIGGER_ID, PLACEMENT_ID), Some(OTHER_OWNER)),
    )
    .await;
    let missing_trigger = json_response(
        &app,
        request(embed_path("trg_missing", PLACEMENT_ID), Some(OWNER)),
    )
    .await;
    let missing_placement = json_response(
        &app,
        request(embed_path(TRIGGER_ID, "plc_missing"), Some(OWNER)),
    )
    .await;
    for response in [&wrong_owner, &missing_trigger, &missing_placement] {
        assert_eq!(response.0, StatusCode::NOT_FOUND);
        assert_eq!(response.2["code"], "trigger_embed_not_found");
        assert_eq!(response.1[header::CACHE_CONTROL], "private, no-store");
    }
    assert_eq!(wrong_owner.2, missing_trigger.2);
    assert_eq!(wrong_owner.2, missing_placement.2);
}

#[tokio::test]
async fn draft_retired_and_retired_placement_descriptors_are_refused() {
    for (fixture, expected_status, expected_code) in [
        (
            Fixture::draft(),
            StatusCode::CONFLICT,
            "trigger_not_published",
        ),
        (
            Fixture::retired_trigger(),
            StatusCode::CONFLICT,
            "trigger_not_published",
        ),
        (
            Fixture::retired_placement(),
            StatusCode::NOT_FOUND,
            "trigger_embed_not_found",
        ),
    ] {
        let (status, headers, value) = json_response(
            &fixture.router(),
            request(embed_path(TRIGGER_ID, PLACEMENT_ID), Some(OWNER)),
        )
        .await;
        assert_eq!(status, expected_status);
        assert_eq!(value["code"], expected_code);
        assert_eq!(headers[header::CACHE_CONTROL], "private, no-store");
    }
}

#[tokio::test]
async fn malformed_descriptor_queries_are_fixed_and_private() {
    let fixture = Fixture::published();
    let app = fixture.router();
    for path in [
        format!("/api/v1/triggers/{TRIGGER_ID}/embed"),
        format!(
            "/api/v1/triggers/{TRIGGER_ID}/embed?placement_id={PLACEMENT_ID}&extra={OWNER_SECRET}"
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(path, Some(OWNER)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_private_headers(&response);
        assert!(!body(response).await.contains(OWNER_SECRET));
    }
}
