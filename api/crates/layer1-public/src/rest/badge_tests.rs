use std::path::PathBuf;

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
};
use layer2_core::trigger_domain::{ButtonLabel, Theme};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{
    badge::{badge_etag, render_badge, BadgeAvailability, BadgeLabel, BadgeSpec},
    trigger_assets_test_support::{Fixture, OWNER_SECRET, PLACEMENT_ID, PUBLIC_ID, TRIGGER_ID},
};

const FORBIDDEN_SVG: &[&str] = &["<script", "<foreignObject", "xlink:href", "@import", "url("];

fn label_cases() -> [(ButtonLabel, &'static str); 4] {
    [
        (ButtonLabel::OpenInCma, "open_in_cma"),
        (ButtonLabel::ChatWithThis, "chat_with_this"),
        (ButtonLabel::RunWithCma, "run_with_cma"),
        (ButtonLabel::TryIt, "try_it"),
    ]
}

fn theme_cases() -> [(Theme, &'static str); 3] {
    [
        (Theme::Auto, "auto"),
        (Theme::Light, "light"),
        (Theme::Dark, "dark"),
    ]
}

fn availability_cases() -> [(BadgeAvailability, &'static str); 2] {
    [
        (BadgeAvailability::Available, "available"),
        (BadgeAvailability::Unavailable, "unavailable"),
    ]
}

#[test]
fn etag_is_the_sha256_of_the_resolved_revision_label_theme_and_availability() {
    let spec = BadgeSpec {
        label: BadgeLabel::Trigger(ButtonLabel::RunWithCma),
        theme: Theme::Dark,
        availability: BadgeAvailability::Unavailable,
    };
    let tuple = b"revision-digest\0run_with_cma\0dark\0unavailable\0";
    assert_eq!(
        badge_etag("revision-digest", spec),
        format!("\"{:x}\"", Sha256::digest(tuple))
    );
    for changed in [
        badge_etag("another-revision", spec),
        badge_etag(
            "revision-digest",
            BadgeSpec {
                label: BadgeLabel::Trigger(ButtonLabel::TryIt),
                ..spec
            },
        ),
        badge_etag(
            "revision-digest",
            BadgeSpec {
                theme: Theme::Light,
                ..spec
            },
        ),
        badge_etag(
            "revision-digest",
            BadgeSpec {
                availability: BadgeAvailability::Available,
                ..spec
            },
        ),
    ] {
        assert_ne!(changed, badge_etag("revision-digest", spec));
    }
}

#[test]
fn every_label_theme_and_availability_has_an_exact_svg_golden() {
    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trigger-badges");
    let update = std::env::var_os("UPDATE_BADGE_GOLDENS").is_some();
    if update {
        std::fs::create_dir_all(&fixture_dir).expect("golden directory is writable");
    }

    for (label, label_name) in label_cases() {
        for (theme, theme_name) in theme_cases() {
            for (availability, availability_name) in availability_cases() {
                let actual = render_badge(BadgeSpec {
                    label: BadgeLabel::Trigger(label),
                    theme,
                    availability,
                });
                let path =
                    fixture_dir.join(format!("{label_name}-{theme_name}-{availability_name}.svg"));
                if update {
                    std::fs::write(&path, &actual).expect("golden SVG is writable");
                }
                let expected = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                assert_eq!(actual, expected, "golden {}", path.display());
                assert!(actual.len() <= 4_096, "{} is too large", path.display());
                assert!(actual.contains("role=\"img\""));
                assert!(actual.contains("<title id=\"cma-title\">"));
                assert!(actual.contains("textLength=\""));
                for forbidden in FORBIDDEN_SVG {
                    assert!(
                        !actual.contains(forbidden),
                        "{} contains {forbidden}",
                        path.display()
                    );
                }
            }
        }
    }
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 4_096)
            .await
            .expect("badge body is bounded")
            .to_vec(),
    )
    .expect("badge body is UTF-8")
}

fn badge_path(placement: &str) -> String {
    format!(
        "/api/v1/triggers/public/{PUBLIC_ID}/badge.svg?label=chat_with_this&theme=dark&placement={placement}"
    )
}

#[tokio::test]
async fn known_badges_have_exact_cache_and_security_headers_and_support_304() {
    let fixture = Fixture::published();
    let app = fixture.router();
    let path = badge_path(PLACEMENT_ID);
    let response = app
        .clone()
        .oneshot(Request::get(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "image/svg+xml; charset=utf-8"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=300, stale-while-revalidate=3600"
    );
    assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'none'; style-src 'unsafe-inline'"
    );
    let etag = response.headers()[header::ETAG].clone();
    let etag_text = etag.to_str().unwrap();
    assert_eq!(etag_text.len(), 66);
    assert!(etag_text.starts_with('"') && etag_text.ends_with('"'));
    let svg = body(response).await;
    assert!(svg.contains("<title id=\"cma-title\">Chat with this</title>"));
    assert!(svg.contains("#0d1117"));
    assert!(!svg.contains(OWNER_SECRET));

    let response = app
        .oneshot(
            Request::get(path)
                .header(header::IF_NONE_MATCH, format!("\"other\", W/{etag_text}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert!(body(response).await.is_empty());
}

#[tokio::test]
async fn ambient_cookies_do_not_change_badge_bytes_or_etag() {
    let fixture = Fixture::published();
    let app = fixture.router();
    let path = badge_path(PLACEMENT_ID);
    let plain = app
        .clone()
        .oneshot(Request::get(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let plain_etag = plain.headers()[header::ETAG].clone();
    let plain_body = body(plain).await;
    let with_cookie = app
        .oneshot(
            Request::get(path)
                .header(header::COOKIE, "session=ambient-private-value")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_cookie.headers()[header::ETAG], plain_etag);
    assert_eq!(body(with_cookie).await, plain_body);
}

#[tokio::test]
async fn retired_or_missing_placements_and_retired_triggers_are_unavailable() {
    for (fixture, placement) in [
        (Fixture::retired_placement(), PLACEMENT_ID),
        (Fixture::published(), "plc_missing"),
        (Fixture::retired_trigger(), PLACEMENT_ID),
    ] {
        let response = fixture
            .router()
            .oneshot(
                Request::get(badge_path(placement))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=300, stale-while-revalidate=3600"
        );
        let svg = body(response).await;
        assert!(svg.contains("<title id=\"cma-title\">Unavailable</title>"));
        assert!(svg.contains("#21262d"));
        assert!(!svg.contains(OWNER_SECRET));
    }
}

#[tokio::test]
async fn unknown_and_malformed_ids_return_a_short_lived_generic_badge() {
    let fixture = Fixture::published();
    let app = fixture.router();
    for id in ["pub_missing", "not.valid"] {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/v1/triggers/public/{id}/badge.svg?label=try_it&theme=light"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=60"
        );
        let svg = body(response).await;
        assert!(svg.contains("<title id=\"cma-title\">CMA</title>"));
        assert!(svg.contains(">CMA</text>"));
        assert!(!svg.contains("Try it"));
        assert!(!svg.contains(OWNER_SECRET));
    }
    assert_eq!(fixture.store.public_lookups(), 1);
}

#[tokio::test]
async fn invalid_query_values_are_closed_and_never_reflected() {
    let fixture = Fixture::published();
    let app = fixture.router();
    for (query, submitted) in [
        ("label=owner_supplied", "owner_supplied"),
        ("theme=sepia", "sepia"),
        ("placement=%3Cscript%3E", "script"),
        ("extra=private_query", "private_query"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/v1/triggers/public/{PUBLIC_ID}/badge.svg?{query}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response_body = body(response).await;
        assert!(!response_body.contains(submitted));
        assert!(!response_body.contains(OWNER_SECRET));
    }
}

#[test]
fn public_origin_and_identifier_validation_are_exact() {
    let fixture = Fixture::published();
    let store: std::sync::Arc<dyn layer2_core::trigger_domain::TriggerStore> =
        fixture.store.clone();
    for invalid in [
        "ftp://cma.example",
        "https://user@cma.example",
        "https://cma.example/path",
        "https://cma.example?query",
        "https://cma.example#fragment",
    ] {
        assert!(super::TriggerAssetsState::new(store.clone(), invalid).is_err());
    }
    let state = super::TriggerAssetsState::new(store, "https://CMA.example:443/").unwrap();
    assert_eq!(state.public_origin().as_str(), "https://cma.example/");
    assert!(super::is_embed_identifier(TRIGGER_ID));
    assert!(!super::is_embed_identifier("contains.dot"));
    assert!(!super::is_embed_identifier(&"a".repeat(129)));
}

struct ErasingPublisher;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ErasingPublisher {
    async fn erasing(
        &self,
        _: &layer2_core::SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
async fn deleting_publisher_badge_and_open_are_unavailable_but_intent_only_clears_cookie() {
    let fixture = Fixture::no_review();
    let state = super::TriggerAssetsState::new(
        fixture.store.clone(),
        super::trigger_assets_test_support::ORIGIN,
    )
    .unwrap()
    .with_account_standing(std::sync::Arc::new(ErasingPublisher));
    let router = super::trigger_assets_router(state);
    let badge = router
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/triggers/public/{PUBLIC_ID}/badge.svg"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(badge.status(), StatusCode::OK);
    let svg = String::from_utf8(to_bytes(badge.into_body(), 4096).await.unwrap().to_vec()).unwrap();
    assert!(svg.contains("Unavailable"));
    let entry = router
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/v1/triggers/public/{PUBLIC_ID}/open/{PLACEMENT_ID}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(entry.status(), StatusCode::SERVICE_UNAVAILABLE);
    let consume = router
        .oneshot(
            Request::post(format!("/api/v1/triggers/public/{PUBLIC_ID}/entry-intent"))
                .header(header::ORIGIN, super::trigger_assets_test_support::ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(consume.status(), StatusCode::OK);
    assert_eq!(
        consume.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    assert_eq!(consume.headers()[header::REFERRER_POLICY], "no-referrer");
}
