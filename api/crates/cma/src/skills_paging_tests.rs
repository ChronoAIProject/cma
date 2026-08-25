//! Contract tests for Ornn's cursor-paged skill catalogue.

use std::collections::VecDeque;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Extension, Json, Router};
use layer1_public::{auth::DelegatedCredential, AuthenticatedSubject};
use layer2_core::{
    AgentConfiguration, Authorization, Baseline, Capability, EnvironmentProfile,
    EnvironmentProfileId, EnvironmentProfileService, EnvironmentProfileSpec,
    EnvironmentProfileStore, Page, PageRequest, PlatformSettingsService, StoreError,
};
use tokio::sync::Mutex;
use tower::ServiceExt;

use super::*;
use crate::store::token::StaticToken;

const SEARCH_FIXTURE_PATH: &str = "/api/v1/proxy/s/ornn-api/api/v1/skill-search";
const DETAIL_FIXTURE_PREFIX: &str = "/api/v1/proxy/s/ornn-api/api/v1/skills/";

#[derive(Debug)]
struct RecordedRequest {
    uri: String,
    authorization: Option<String>,
}

struct Reply {
    path: Option<String>,
    query: Option<String>,
    status: StatusCode,
    body: serde_json::Value,
}

impl Reply {
    fn ok(body: serde_json::Value) -> Self {
        Self {
            path: None,
            query: None,
            status: StatusCode::OK,
            body,
        }
    }

    fn search_status(status: StatusCode, body: serde_json::Value) -> Self {
        Self {
            path: Some(SEARCH_FIXTURE_PATH.to_string()),
            query: None,
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

fn query_from_uri(uri: &str, key: &str) -> Option<String> {
    reqwest::Url::parse(&format!("http://registry.test{uri}"))
        .expect("fixture recorded a path and query")
        .query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
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
    let reply_index = replies.iter().position(|reply| {
        reply.path.as_ref().is_none_or(|path| path == &request_path)
            && reply.query.as_ref().is_none_or(|query| {
                query_from_uri(&request.uri().to_string(), "q").as_deref() == Some(query.as_str())
            })
    });
    let reply = reply_index
        .and_then(|index| replies.remove(index))
        .unwrap_or_else(|| Reply {
            path: None,
            query: None,
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

struct EmptyProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for EmptyProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        Ok(())
    }

    async fn replace_content(
        &self,
        _id: &EnvironmentProfileId,
        _content: &EnvironmentProfileSpec,
        _updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(
        &self,
        _owner: &SubjectId,
        _page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn set_preferred(
        &self,
        _owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        if chosen.is_some() {
            return Err(StoreError::Malformed(
                "an empty store has no selectable profile".into(),
            ));
        }
        Ok(())
    }

    async fn preferred_for(
        &self,
        _owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        Ok(())
    }
}

fn public_skills_router(catalog: PlatformSkillCatalog) -> Router {
    let environments = EnvironmentProfileService::new(
        Arc::new(EmptyProfiles),
        Baseline::new([], [], []),
        Some(Arc::new(catalog)),
        None,
        AgentConfiguration::default(),
        Arc::new(PlatformSettingsService::unconfigured()),
    );
    layer1_public::rest::environments::router(environments)
        .layer(Extension(DelegatedCredential::subject(
            "caller-token".into(),
        )))
        .layer(Extension(Authorization::from_capabilities([
            Capability::ManageOwnEnvironmentProfiles,
        ])))
        .layer(Extension(AuthenticatedSubject(SubjectId(
            "subject-a".into(),
        ))))
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
    query_from_uri(&request.uri, key)
}

fn assert_catalogue_search(request: &RecordedRequest) {
    let parsed = parsed(request);
    assert_eq!(parsed.path(), SEARCH_FIXTURE_PATH);
    assert!(!parsed.path().starts_with(DETAIL_FIXTURE_PREFIX));
    assert_eq!(query(request, "q"), None);
    assert_eq!(query(request, "query"), None, "legacy query is not emitted");
    assert_eq!(query(request, "scope").as_deref(), Some(SCOPE));
    assert_eq!(query(request, "mode").as_deref(), Some("keyword"));
    assert_eq!(query(request, "limit").as_deref(), Some("100"));
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
async fn explicit_empty_items_is_a_valid_empty_catalogue() {
    let (base, requests, server) = registry(vec![Reply::ok(page(Vec::new(), false, None))]).await;

    let listed = catalog(&base)
        .available(&SubjectId("subject-a".into()), None)
        .await
        .expect("an explicit empty item list is complete");

    assert!(listed.is_empty());
    assert_eq!(requests.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn missing_items_is_malformed_and_publicly_redacted_as_a_coded_503() {
    const PRIVATE_UPSTREAM_MARKER: &str = "private-upstream-diagnostic";
    let malformed = serde_json::json!({
        "data": {
            "meta": {
                "limit": PAGE_SIZE,
                "hasMore": false
            },
            "diagnostic": PRIVATE_UPSTREAM_MARKER
        },
        "error": null
    });
    let (base, requests, server) =
        registry(vec![Reply::ok(malformed.clone()), Reply::ok(malformed)]).await;

    let error = catalog(&base)
        .available(&SubjectId("subject-a".into()), Some("caller-token"))
        .await
        .expect_err("absence is not an explicitly empty catalogue");
    assert!(matches!(error, CatalogError::Malformed(_)), "got {error:?}");

    let response = public_skills_router(catalog(&base))
        .oneshot(
            Request::get("/api/v1/environment-profiles/skills")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("public router answers");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        ))
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("problem body is bounded");
    let problem: serde_json::Value = serde_json::from_slice(&body).expect("problem is JSON");
    assert_eq!(problem["code"], "skill_catalogue_unavailable");
    assert_eq!(problem["status"], 503);
    assert_eq!(
        problem["detail"],
        "The skill catalogue could not be read completely."
    );
    assert!(!String::from_utf8_lossy(&body).contains(PRIVATE_UPSTREAM_MARKER));
    assert_eq!(requests.lock().await.len(), 2);
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
async fn an_exact_slug_after_more_than_one_hundred_rows_is_described_without_detail_gets() {
    let mut second = rows(PAGE_SIZE, 2);
    let exact = serde_json::json!({
        "guid": "guid-outside",
        "name": "outside-first-page",
        "description": "Description from side-effect-free search"
    });
    second.push(exact);
    let (base, requests, server) = registry(vec![
        Reply::ok(page(
            rows(0, PAGE_SIZE),
            true,
            Some("description-page-two".into()),
        )),
        Reply::ok(page(second, false, None)),
    ])
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
        .expect("targeted metadata search");

    assert_eq!(described.len(), 1, "duplicate grants are fetched once");
    assert_eq!(described[0].requested.0, "outside-first-page");
    assert_eq!(
        described[0].skill.description,
        "Description from side-effect-free search"
    );
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(query(&requests[0], "cursor"), None);
    assert_eq!(
        query(&requests[1], "cursor").as_deref(),
        Some("description-page-two")
    );
    assert!(requests.iter().all(|request| {
        assert_catalogue_search(request);
        request.authorization.as_deref() == Some("Bearer caller-token")
    }));
    server.abort();
}

#[tokio::test]
async fn fuzzy_only_search_hits_never_enrich_a_recorded_grant() {
    let fuzzy = serde_json::json!({
        "guid": "guid-fuzzy",
        "name": "requested-skill-helper",
        "description": "Mentions requested-skill but is not it"
    });
    let (base, requests, server) = registry(vec![Reply::ok(page(vec![fuzzy], false, None))]).await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[SkillId::new("requested-skill")],
        )
        .await
        .expect("a terminal fuzzy-only page means the exact grant is absent");

    assert!(described.is_empty());
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_catalogue_search(&requests[0]);
    server.abort();
}

#[tokio::test]
async fn one_missing_grant_does_not_hide_another_grants_description() {
    let (base, requests, server) = registry(vec![Reply::ok(page(
        vec![serde_json::json!({
            "guid": "guid-visible",
            "name": "visible-skill",
            "description": "This description remains available"
        })],
        false,
        None,
    ))])
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
    assert_eq!(described[0].requested.0, "visible-skill");
    assert_eq!(described[0].skill.name, "visible-skill");
    assert_eq!(
        described[0].skill.description,
        "This description remains available"
    );
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_catalogue_search(&requests[0]);
    server.abort();
}

#[tokio::test]
async fn a_non_successful_metadata_search_remains_a_loud_catalogue_error() {
    const PRIVATE_UPSTREAM_MARKER: &str = "provider-body-must-not-be-consumed";
    let (base, requests, server) = registry(vec![Reply::search_status(
        StatusCode::BAD_GATEWAY,
        serde_json::json!({ "error": PRIVATE_UPSTREAM_MARKER }),
    )])
    .await;

    let error = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[SkillId::new("unavailable-skill")],
        )
        .await
        .expect_err("a failed search is not proof that the grant is missing");

    assert!(
        matches!(error, CatalogError::Unavailable(_)),
        "got {error:?}"
    );
    assert!(error.to_string().contains("502 Bad Gateway"));
    assert!(!error.to_string().contains(PRIVATE_UPSTREAM_MARKER));
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_catalogue_search(&requests[0]);
    server.abort();
}

#[tokio::test]
async fn an_ambiguous_slug_does_not_hide_a_separately_requested_guid() {
    let (base, requests, server) = registry(vec![Reply::ok(page(
        vec![
            serde_json::json!({
                "guid": "guid-a",
                "name": "shared",
                "description": "first shared row"
            }),
            serde_json::json!({
                "guid": "guid-b",
                "name": "shared",
                "description": "second shared row"
            }),
            serde_json::json!({
                "guid": "guid-visible",
                "name": "visible",
                "description": "independent metadata"
            }),
        ],
        false,
        None,
    ))])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[
                SkillId::new("guid-a"),
                SkillId::new("shared"),
                SkillId::new("guid-visible"),
            ],
        )
        .await
        .expect("ambiguity is isolated to its requested identifier");

    assert_eq!(
        described
            .iter()
            .map(|description| description.requested.0.as_str())
            .collect::<Vec<_>>(),
        ["guid-a", "guid-visible"]
    );
    assert_eq!(described[0].skill.name, "shared");
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_catalogue_search(&requests[0]);
    server.abort();
}

#[tokio::test]
async fn a_later_transport_failure_preserves_prior_exact_guid_metadata() {
    const PRIVATE_UPSTREAM_MARKER: &str = "transport-body-must-not-be-consumed";
    let (base, requests, server) = registry(vec![
        Reply::ok(page(
            vec![serde_json::json!({
                "guid": "guid-safe",
                "name": "safe-slug",
                "description": "metadata collected before the outage"
            })],
            true,
            Some("failing-page-two".into()),
        )),
        Reply::search_status(
            StatusCode::BAD_GATEWAY,
            serde_json::json!({ "error": PRIVATE_UPSTREAM_MARKER }),
        ),
    ])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[
                SkillId::new("guid-safe"),
                SkillId::new("safe-slug"),
                SkillId::new("unresolved-after-outage"),
            ],
        )
        .await
        .expect("an exact GUID found before a later outage remains safe");

    assert_eq!(described.len(), 1);
    assert_eq!(described[0].requested.0, "guid-safe");
    assert!(described
        .iter()
        .all(|description| description.requested.0 != "safe-slug"));
    assert_eq!(
        described[0].skill.description,
        "metadata collected before the outage"
    );
    assert!(!format!("{described:?}").contains(PRIVATE_UPSTREAM_MARKER));
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_catalogue_search(request);
    }
    server.abort();
}

#[tokio::test]
async fn a_malformed_later_page_cannot_contribute_metadata_from_that_page() {
    let (base, requests, server) = registry(vec![
        Reply::ok(page(
            vec![serde_json::json!({
                "guid": "guid-prior",
                "name": "prior-skill",
                "description": "metadata from a validated page"
            })],
            true,
            Some("malformed-page-two".into()),
        )),
        Reply::ok(serde_json::json!({
            "data": {
                "items": [{
                    "guid": "guid-on-malformed-page",
                    "name": "malformed-page-skill",
                    "description": "must be discarded with its page"
                }]
            },
            "error": null
        })),
    ])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[
                SkillId::new("guid-prior"),
                SkillId::new("guid-on-malformed-page"),
            ],
        )
        .await
        .expect("only metadata from the validated page survives");

    assert_eq!(described.len(), 1);
    assert_eq!(described[0].requested.0, "guid-prior");
    assert_eq!(
        described[0].skill.description,
        "metadata from a validated page"
    );
    assert!(described
        .iter()
        .all(|description| description.requested.0 != "guid-on-malformed-page"));
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_catalogue_search(request);
    }
    server.abort();
}

#[tokio::test]
async fn one_malformed_identifier_does_not_discard_valid_metadata() {
    let (base, requests, server) = registry(vec![Reply::ok(page(
        vec![
            serde_json::json!({
                "guid": "",
                "name": "malformed-skill",
                "description": "must never attach"
            }),
            serde_json::json!({
                "guid": "guid-valid",
                "name": "valid-skill",
                "description": "valid metadata survives"
            }),
        ],
        false,
        None,
    ))])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[SkillId::new("malformed-skill"), SkillId::new("valid-skill")],
        )
        .await
        .expect("one malformed candidate is isolated");

    assert_eq!(described.len(), 1);
    assert_eq!(described[0].requested.0, "valid-skill");
    assert_eq!(described[0].skill.description, "valid metadata survives");
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_catalogue_search(&requests[0]);
    server.abort();
}

#[tokio::test]
async fn one_hundred_caller_skills_plus_the_mandatory_skill_are_all_described() {
    let mut items = vec![serde_json::json!({
        "guid": "guid-mandatory-skill",
        "name": "mandatory-skill",
        "description": "mandatory metadata"
    })];
    items.extend((0..100).map(|number| {
        serde_json::json!({
            "guid": format!("guid-caller-skill-{number}"),
            "name": format!("caller-skill-{number}"),
            "description": format!("caller metadata {number}")
        })
    }));
    let second_page = items.split_off(PAGE_SIZE);
    let (base, requests, server) = registry(vec![
        Reply::ok(page(items, true, Some("boundary-page-two".into()))),
        Reply::ok(page(second_page, false, None)),
    ])
    .await;

    let mut requested = vec![SkillId::new("mandatory-skill")];
    requested.extend((0..100).map(|number| SkillId::new(format!("caller-skill-{number}"))));

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &requested,
        )
        .await
        .expect("the valid profile boundary is fully eligible for enrichment");

    assert_eq!(described.len(), 101);
    assert_eq!(described[0].requested.0, "mandatory-skill");
    assert_eq!(described[0].skill.description, "mandatory metadata");
    for (number, description) in described.iter().skip(1).enumerate() {
        assert_eq!(description.requested.0, format!("caller-skill-{number}"));
        assert_eq!(description.skill.name, format!("caller-skill-{number}"));
        assert_eq!(
            description.skill.description,
            format!("caller metadata {number}")
        );
    }
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 2, "one search walk serves all 101 grants");
    assert_eq!(
        query(&requests[1], "cursor").as_deref(),
        Some("boundary-page-two")
    );
    for request in requests.iter() {
        assert_catalogue_search(request);
    }
    server.abort();
}

#[tokio::test]
async fn description_enrichment_never_calls_the_pull_counting_detail_route() {
    let detail_path = format!("{DETAIL_FIXTURE_PREFIX}tempting-skill");
    let tempting_detail_reply = Reply {
        path: Some(detail_path),
        query: None,
        status: StatusCode::OK,
        body: serde_json::json!({
            "guid": "guid-tempting",
            "name": "tempting-skill",
            "description": "detail metadata must remain untouched"
        }),
    };
    let (base, requests, server) = registry(vec![
        tempting_detail_reply,
        Reply::ok(page(
            vec![serde_json::json!({
                "guid": "guid-tempting",
                "name": "tempting-skill",
                "description": "side-effect-free search metadata"
            })],
            false,
            None,
        )),
    ])
    .await;

    let described = catalog(&base)
        .describe(
            &SubjectId("subject-a".into()),
            Some("caller-token"),
            &[SkillId::new("tempting-skill")],
        )
        .await
        .expect("search metadata resolves without materializing the package");

    assert_eq!(described.len(), 1);
    assert_eq!(
        described[0].skill.description,
        "side-effect-free search metadata"
    );
    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests
            .iter()
            .filter(|request| parsed(request).path().starts_with(DETAIL_FIXTURE_PREFIX))
            .count(),
        0,
        "authenticated detail GETs increment Ornn pull counts"
    );
    assert_catalogue_search(&requests[0]);
    server.abort();
}
