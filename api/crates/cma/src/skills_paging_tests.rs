//! Contract tests for Ornn's cursor-paged skill catalogue.

use std::collections::VecDeque;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Json, Router};
use tokio::sync::Mutex;

use super::*;
use crate::store::token::StaticToken;

#[derive(Debug)]
struct RecordedRequest {
    uri: String,
    authorization: Option<String>,
}

struct Reply {
    path: Option<String>,
    status: StatusCode,
    body: serde_json::Value,
}

impl Reply {
    fn ok(body: serde_json::Value) -> Self {
        Self {
            path: None,
            status: StatusCode::OK,
            body,
        }
    }

    fn at(path: &str, status: StatusCode, body: serde_json::Value) -> Self {
        Self {
            path: Some(path.to_string()),
            status,
            body,
        }
    }
}

#[derive(Clone)]
struct RegistryFixture {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

async fn answer(State(fixture): State<RegistryFixture>, request: Request) -> impl IntoResponse {
    let request_path = request.uri().path().to_string();
    fixture.requests.lock().await.push(RecordedRequest {
        uri: request.uri().to_string(),
        authorization: request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    });
    let mut replies = fixture.replies.lock().await;
    let reply_index = replies
        .iter()
        .position(|reply| reply.path.as_ref().is_none_or(|path| path == &request_path));
    let reply = reply_index
        .and_then(|index| replies.remove(index))
        .unwrap_or_else(|| Reply {
            path: None,
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: serde_json::json!({ "error": "unexpected request" }),
        });
    (reply.status, Json(reply.body))
}

async fn registry(
    replies: Vec<Reply>,
) -> (
    String,
    Arc<Mutex<Vec<RecordedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let fixture = RegistryFixture {
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let requests = Arc::clone(&fixture.requests);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind registry fixture");
    let address = listener.local_addr().expect("fixture address");
    let app = Router::new().fallback(any(answer)).with_state(fixture);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve registry fixture");
    });
    (format!("http://{address}"), requests, server)
}

fn catalog(base: &str) -> PlatformSkillCatalog {
    PlatformSkillCatalog::new(base, Arc::new(StaticToken("service-token".into())))
}

fn row(number: usize) -> serde_json::Value {
    serde_json::json!({
        "guid": format!("guid-{number}"),
        "name": format!("skill-{number}"),
        "description": format!("description {number}"),
        "myAccessReason": "public"
    })
}

fn rows(start: usize, count: usize) -> Vec<serde_json::Value> {
    (start..start + count).map(row).collect()
}

fn detail_replies(count: usize) -> Vec<Reply> {
    (0..count)
        .map(|number| {
            Reply::at(
                &format!("/api/v1/proxy/s/ornn-api/api/v1/skills/skill-{number}"),
                StatusCode::OK,
                serde_json::json!({
                    "data": row(number),
                    "error": null
                }),
            )
        })
        .collect()
}

fn skill_ids(count: usize) -> Vec<SkillId> {
    (0..count)
        .map(|number| SkillId::new(format!("skill-{number}")))
        .collect()
}

fn page(
    items: Vec<serde_json::Value>,
    has_more: bool,
    next_cursor: Option<String>,
) -> serde_json::Value {
    let mut meta = serde_json::json!({
        "limit": PAGE_SIZE,
        "hasMore": has_more
    });
    if let Some(next_cursor) = next_cursor {
        meta["nextCursor"] = serde_json::Value::String(next_cursor);
    }
    serde_json::json!({
        "data": {
            "items": items,
            "meta": meta
        },
        "error": null
    })
}

fn parsed(request: &RecordedRequest) -> reqwest::Url {
    reqwest::Url::parse(&format!("http://registry.test{}", request.uri))
        .expect("fixture recorded a path and query")
}

fn query(request: &RecordedRequest, key: &str) -> Option<String> {
    parsed(request)
        .query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

#[tokio::test]
async fn a_cursor_walk_reaches_skills_after_the_first_page() {
    let mut second = rows(PAGE_SIZE, 1);
    second[0]["name"] = serde_json::Value::String("outside-first-page".into());
    second[0]["description"] = serde_json::Value::String("Found on page two".into());
    let (base, requests, server) = registry(vec![
        Reply::ok(page(
            rows(0, PAGE_SIZE),
            true,
            Some("opaque-page-two".into()),
        )),
        Reply::ok(page(second, false, None)),
    ])
    .await;

    let listed = catalog(&base)
        .available(&SubjectId("subject-a".into()), Some("caller-token"))
        .await
        .expect("complete catalogue");

    assert_eq!(listed.len(), PAGE_SIZE + 1);
    assert_eq!(listed[PAGE_SIZE].name, "outside-first-page");
    assert_eq!(listed[PAGE_SIZE].description, "Found on page two");
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(query(&requests[0], "limit").as_deref(), Some("100"));
    assert_eq!(query(&requests[0], "cursor"), None);
    assert_eq!(
        query(&requests[1], "cursor").as_deref(),
        Some("opaque-page-two")
    );
    assert!(requests
        .iter()
        .all(|request| { request.authorization.as_deref() == Some("Bearer caller-token") }));
    server.abort();
}

#[tokio::test]
async fn a_terminal_full_page_ignores_the_registrys_unused_cursor() {
    // Ornn can emit nextCursor for any full page, including the final one.
    // hasMore is authoritative; following this cursor would make one needless
    // request and misread its empty response as a catalogue failure.
    let (base, requests, server) = registry(vec![Reply::ok(page(
        rows(0, PAGE_SIZE),
        false,
        Some("not-a-continuation".into()),
    ))])
    .await;

    let listed = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect("hasMore=false ends the walk");

    assert_eq!(listed.len(), PAGE_SIZE);
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn a_terminal_page_at_the_exact_catalogue_bound_succeeds() {
    let replies = (0..CATALOG_PAGE_LIMIT)
        .map(|page_number| {
            let has_more = page_number + 1 < CATALOG_PAGE_LIMIT;
            Reply::ok(page(
                rows(page_number * PAGE_SIZE, PAGE_SIZE),
                has_more,
                has_more.then(|| format!("cursor-{}", page_number + 2)),
            ))
        })
        .collect();
    let (base, requests, server) = registry(replies).await;

    let listed = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect("page fifty is terminal");

    assert_eq!(listed.len(), CATALOG_SKILL_LIMIT);
    assert_eq!(listed.last().expect("last skill").id.0, "guid-4999");
    assert_eq!(requests.lock().await.len(), CATALOG_PAGE_LIMIT);
    server.abort();
}

#[tokio::test]
async fn another_page_after_the_bound_fails_instead_of_returning_five_thousand() {
    let replies = (0..CATALOG_PAGE_LIMIT)
        .map(|page_number| {
            Reply::ok(page(
                rows(page_number * PAGE_SIZE, PAGE_SIZE),
                true,
                Some(format!("cursor-{}", page_number + 2)),
            ))
        })
        .collect();
    let (base, requests, server) = registry(replies).await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect_err("a partial catalogue must not escape");

    assert!(matches!(error, CatalogError::Malformed(_)), "got {error:?}");
    assert!(error.to_string().contains("50 pages / 5000 skills"));
    assert_eq!(requests.lock().await.len(), CATALOG_PAGE_LIMIT);
    server.abort();
}

#[tokio::test]
async fn a_looping_cursor_is_refused_before_a_third_request() {
    let (base, requests, server) = registry(vec![
        Reply::ok(page(rows(0, 1), true, Some("same-cursor".into()))),
        Reply::ok(page(rows(1, 1), true, Some("same-cursor".into()))),
    ])
    .await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect_err("cursor loop must fail");

    assert!(error.to_string().contains("repeated a cursor"));
    assert_eq!(requests.lock().await.len(), 2);
    server.abort();
}

#[tokio::test]
async fn has_more_without_a_cursor_is_not_read_as_a_complete_catalogue() {
    let (base, requests, server) = registry(vec![Reply::ok(page(rows(0, 1), true, None))]).await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect_err("missing cursor must fail");

    assert!(error.to_string().contains("has no next cursor"));
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn blank_and_oversized_continuation_cursors_are_rejected() {
    for (cursor, expected) in [
        ("   ".to_string(), "has no next cursor"),
        ("x".repeat(CURSOR_LENGTH_LIMIT + 1), "oversized cursor"),
    ] {
        let (base, requests, server) =
            registry(vec![Reply::ok(page(rows(0, 1), true, Some(cursor)))]).await;

        let error = catalog(&base)
            .available(&SubjectId("subject-a".into()), None)
            .await
            .expect_err("an unusable cursor must fail");

        assert!(matches!(error, CatalogError::Malformed(_)), "got {error:?}");
        assert!(error.to_string().contains(expected), "got {error}");
        assert_eq!(requests.lock().await.len(), 1);
        server.abort();
    }
}

#[tokio::test]
async fn invalid_limits_and_pages_larger_than_the_reported_limit_are_rejected() {
    for (limit, item_count) in [(0, 1), (PAGE_SIZE + 1, 1), (1, 2)] {
        let mut body = page(rows(0, item_count), false, None);
        body["data"]["meta"]["limit"] = serde_json::json!(limit);
        let (base, requests, server) = registry(vec![Reply::ok(body)]).await;

        let error = catalog(&base)
            .available(&SubjectId("subject-a".into()), None)
            .await
            .expect_err("an invalid page contract must fail");

        assert!(matches!(error, CatalogError::Malformed(_)), "got {error:?}");
        assert_eq!(requests.lock().await.len(), 1);
        server.abort();
    }
}

#[tokio::test]
async fn items_without_a_non_blank_identity_and_name_are_rejected() {
    let mut blank_guid = row(0);
    blank_guid["guid"] = serde_json::Value::String("  ".into());
    let mut blank_name = row(1);
    blank_name["name"] = serde_json::Value::String("  ".into());
    let mut missing_guid = row(2);
    missing_guid
        .as_object_mut()
        .expect("row is an object")
        .remove("guid");
    let mut missing_name = row(3);
    missing_name
        .as_object_mut()
        .expect("row is an object")
        .remove("name");

    for invalid in [blank_guid, blank_name, missing_guid, missing_name] {
        let (base, requests, server) =
            registry(vec![Reply::ok(page(vec![invalid], false, None))]).await;

        let error = catalog(&base)
            .available(&SubjectId("subject-a".into()), None)
            .await
            .expect_err("an item without a durable identity must fail");

        assert!(matches!(error, CatalogError::Malformed(_)), "got {error:?}");
        assert_eq!(requests.lock().await.len(), 1);
        server.abort();
    }
}

#[tokio::test]
async fn a_duplicate_identity_across_pages_is_not_silently_deduplicated() {
    let (base, requests, server) = registry(vec![
        Reply::ok(page(rows(0, 1), true, Some("page-two".into()))),
        Reply::ok(page(rows(0, 1), false, None)),
    ])
    .await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect_err("an unstable walk must fail");

    assert!(error.to_string().contains("repeated identity guid-0"));
    assert_eq!(requests.lock().await.len(), 2);
    server.abort();
}

#[tokio::test]
async fn missing_pagination_metadata_is_a_failure_not_an_empty_or_partial_page() {
    let (base, requests, server) = registry(vec![Reply::ok(serde_json::json!({
        "data": { "items": [row(0)] },
        "error": null
    }))])
    .await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect_err("completeness is unknown");

    assert!(error.to_string().contains("no pagination metadata"));
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn missing_data_is_a_failure_not_an_empty_catalogue() {
    let (base, requests, server) =
        registry(vec![Reply::ok(serde_json::json!({ "error": null }))]).await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect_err("a missing envelope is not an empty account");

    assert!(error.to_string().contains("response has no data"));
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn describing_an_out_of_page_slug_uses_one_targeted_read_and_never_searches() {
    let (base, requests, server) = registry(vec![Reply::ok(serde_json::json!({
        "data": {
            "guid": "guid-outside",
            "name": "outside-first-page",
            "description": "Description from the detail route"
        },
        "error": null
    }))])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[
                SkillId::new("outside-first-page"),
                SkillId::new("outside-first-page"),
            ],
        )
        .await
        .expect("targeted detail");

    assert_eq!(described.len(), 1, "duplicate grants are fetched once");
    assert_eq!(
        described[0].description,
        "Description from the detail route"
    );
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        parsed(&requests[0]).path(),
        "/api/v1/proxy/s/ornn-api/api/v1/skills/outside-first-page"
    );
    assert!(!requests[0].uri.contains("skill-search"));
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer caller-token")
    );
    server.abort();
}

#[tokio::test]
async fn one_missing_grant_does_not_hide_another_grants_description() {
    let missing_path = "/api/v1/proxy/s/ornn-api/api/v1/skills/removed-skill";
    let visible_path = "/api/v1/proxy/s/ornn-api/api/v1/skills/visible-skill";
    let (base, requests, server) = registry(vec![
        Reply::at(
            missing_path,
            StatusCode::NOT_FOUND,
            serde_json::json!({ "code": "skill_not_found" }),
        ),
        Reply::at(
            visible_path,
            StatusCode::OK,
            serde_json::json!({
                "data": {
                    "guid": "guid-visible",
                    "name": "visible-skill",
                    "description": "This description remains available"
                },
                "error": null
            }),
        ),
    ])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[SkillId::new("removed-skill"), SkillId::new("visible-skill")],
        )
        .await
        .expect("one missing row is not a catalogue outage");

    assert_eq!(described.len(), 1);
    assert_eq!(described[0].name, "visible-skill");
    assert_eq!(
        described[0].description,
        "This description remains available"
    );
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .any(|request| parsed(request).path() == missing_path));
    assert!(requests
        .iter()
        .any(|request| parsed(request).path() == visible_path));
    server.abort();
}

#[tokio::test]
async fn description_lookups_below_the_ceiling_call_every_distinct_skill_once() {
    let count = 3;
    let (base, requests, server) = registry(detail_replies(count)).await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &skill_ids(count),
        )
        .await
        .expect("all detail reads fit below the ceiling");

    assert_eq!(requests.lock().await.len(), count);
    assert_eq!(
        described
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        ["skill-0", "skill-1", "skill-2"]
    );
    server.abort();
}

#[tokio::test]
async fn description_lookups_at_the_ceiling_call_every_distinct_skill_once() {
    let (base, requests, server) = registry(detail_replies(DESCRIPTION_LOOKUP_LIMIT)).await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &skill_ids(DESCRIPTION_LOOKUP_LIMIT),
        )
        .await
        .expect("the exact ceiling is accepted");

    assert_eq!(requests.lock().await.len(), DESCRIPTION_LOOKUP_LIMIT);
    assert_eq!(described.len(), DESCRIPTION_LOOKUP_LIMIT);
    assert_eq!(described.first().expect("first").name, "skill-0");
    assert_eq!(
        described.last().expect("last").name,
        format!("skill-{}", DESCRIPTION_LOOKUP_LIMIT - 1)
    );
    server.abort();
}

#[tokio::test]
async fn description_lookups_over_the_ceiling_use_the_first_distinct_skills_only() {
    let (base, requests, server) = registry(detail_replies(DESCRIPTION_LOOKUP_LIMIT)).await;
    let mut requested = vec![
        SkillId::new("skill-0"),
        SkillId::new("skill-1"),
        SkillId::new("skill-0"),
    ];
    requested.extend(
        (2..DESCRIPTION_LOOKUP_LIMIT + 2).map(|number| SkillId::new(format!("skill-{number}"))),
    );

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &requested,
        )
        .await
        .expect("overflow is intentionally left undescribed");

    assert_eq!(
        requests.lock().await.len(),
        DESCRIPTION_LOOKUP_LIMIT,
        "duplicates and overflow must not become provider calls"
    );
    assert_eq!(described.len(), DESCRIPTION_LOOKUP_LIMIT);
    assert_eq!(
        described
            .iter()
            .map(|skill| skill.name.clone())
            .collect::<Vec<_>>(),
        (0..DESCRIPTION_LOOKUP_LIMIT)
            .map(|number| format!("skill-{number}"))
            .collect::<Vec<_>>(),
        "first appearance selects a stable enrichment prefix"
    );
    assert!(!described.iter().any(|skill| {
        skill.name == format!("skill-{DESCRIPTION_LOOKUP_LIMIT}")
            || skill.name == format!("skill-{}", DESCRIPTION_LOOKUP_LIMIT + 1)
    }));
    server.abort();
}
