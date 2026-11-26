use super::*;
use layer2_core::sandbox_usage::UsageEventKind;
use layer2_core::sandbox_usage_cursor::UsageSnapshot;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const OWN: &[Capability] = &[Capability::UseOwnSandboxes];
const ADMIN: &[Capability] = &[Capability::UseOwnSandboxes, Capability::ReadAllSandboxes];
const LIST: &str = "/api/v1/usage/sandboxes";
const ALL: &str = "/api/v1/admin/usage/sandboxes";

#[derive(Default)]
struct Pages {
    lists: AtomicUsize,
    events: AtomicUsize,
    reads: AtomicUsize,
    snapshot_missing: AtomicBool,
}

fn row(id: &str) -> SandboxUsage {
    SandboxUsage {
        sandbox_id: SandboxId(id.into()),
        owner: SubjectId(if id == "sbx_d" { "other" } else { "owner" }.into()),
        name: "synthetic".into(),
        status: "retired".into(),
        allocation: Allocation::default(),
        created_at: 1,
        tracking_started_at: 1,
        last_observed_at: 2,
        source_seq: 3,
        source_revision: 3,
        revision: 3,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    }
}

fn page<T>(mut items: Vec<T>, limit: u16, key: impl Fn(&T) -> String) -> Page<T> {
    let next = if items.len() > usize::from(limit) {
        items.truncate(usize::from(limit));
        items.last().map(key)
    } else {
        None
    };
    Page { items, next }
}

#[async_trait]
impl SandboxUsageStore for Pages {
    async fn usage(&self, id: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(["sbx_a", "sbx_b", "sbx_c", "sbx_d"]
            .contains(&id.as_str())
            .then(|| row(id.as_str())))
    }
    async fn usage_page(
        &self,
        owner: Option<&SubjectId>,
        request: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        assert!(!request
            .cursor
            .as_deref()
            .is_some_and(|key| key.starts_with("u1.")));
        let rows = ["sbx_a", "sbx_b", "sbx_c", "sbx_d"]
            .into_iter()
            .map(row)
            .filter(|row| owner.is_none_or(|owner| owner == &row.owner))
            .filter(|row| {
                request
                    .cursor
                    .as_ref()
                    .is_none_or(|last| row.sandbox_id.as_str() > last.as_str())
            })
            .collect();
        Ok(page(rows, request.limit, |row| row.sandbox_id.to_string()))
    }
    async fn usage_snapshot_page(
        &self,
        owner: Option<&SubjectId>,
        request: &PageRequest,
        snapshot: Option<UsageSnapshot>,
    ) -> Result<(Page<SandboxUsage>, UsageSnapshot), SandboxStoreError> {
        let observed = UsageSnapshot {
            time: 75,
            increment: 1,
        };
        if let Some(expected) = snapshot {
            assert_eq!(expected, observed);
        }
        if self.snapshot_missing.load(Ordering::SeqCst) {
            return Err(SandboxStoreError::InvalidPage);
        }
        Ok((self.usage_page(owner, request).await?, observed))
    }
    async fn usage_summary(
        &self,
        _: Option<&SubjectId>,
        _: u64,
    ) -> Result<UsageSummary, SandboxStoreError> {
        unreachable!()
    }
    async fn usage_events(
        &self,
        id: &SandboxId,
        request: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError> {
        self.events.fetch_add(1, Ordering::SeqCst);
        let last = request
            .cursor
            .as_ref()
            .map(|key| key.parse::<u64>().unwrap())
            .unwrap_or(0);
        let rows = (1..=3)
            .filter(|revision| *revision > last)
            .map(|revision| UsageEvent {
                sandbox_id: id.clone(),
                owner: row(id.as_str()).owner,
                revision,
                source_seq: revision,
                kind: UsageEventKind::Checkpoint,
                started_at: revision,
                ended_at: revision + 1,
                status: "retired".into(),
                allocation: Allocation::default(),
                next_allocation: Allocation::default(),
                quantity: UsageTotals::default(),
            })
            .collect();
        Ok(page(rows, request.limit, |event| {
            event.revision.to_string()
        }))
    }
}

struct Fixture {
    router: Router,
    store: Arc<Pages>,
    clock: Arc<AtomicU64>,
}
impl Fixture {
    fn new(policy: UsageCursorPolicy) -> Self {
        Self::with_snapshots(policy, false)
    }
    fn with_snapshots(policy: UsageCursorPolicy, enabled: bool) -> Self {
        let store = Arc::new(Pages::default());
        let clock = Arc::new(AtomicU64::new(100));
        let time = clock.clone();
        let cursors = UsageCursors::new(&[7; 32], policy)
            .with_snapshot_issuance(enabled)
            .with_clock(Arc::new(move || time.load(Ordering::SeqCst)));
        Self {
            router: sandbox_usage_router(SandboxUsageService::new(store.clone(), cursors)),
            store,
            clock,
        }
    }
    async fn call(
        &self,
        path: &str,
        subject: Option<&str>,
        caps: &[Capability],
        expected: u16,
    ) -> serde_json::Value {
        let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
        if let Some(subject) = subject {
            request
                .extensions_mut()
                .insert(crate::AuthenticatedSubject(SubjectId(subject.into())));
            request
                .extensions_mut()
                .insert(Authorization::new(caps.iter().copied()));
        }
        let response = self.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        if expected == 400 {
            assert_eq!(
                response.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
        }
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
        if expected == 400 {
            assert_eq!(body["code"], "invalid_query");
            assert_eq!(body["detail"], "Usage continuation is invalid or expired. Restart from the first page without a cursor.");
        }
        body
    }
    fn effects(&self) -> (usize, usize) {
        (
            self.store.lists.load(Ordering::SeqCst),
            self.store.events.load(Ordering::SeqCst),
        )
    }
    async fn first(&self, path: &str, caps: &[Capability]) -> String {
        let separator = if path.contains('?') { '&' } else { '?' };
        self.call(
            &format!("{path}{separator}limit=1"),
            Some("owner"),
            caps,
            200,
        )
        .await["next"]
            .as_str()
            .unwrap()
            .into()
    }
}

#[tokio::test]
async fn signed_usage_pages_cover_the_fixed_set_once_and_only_the_real_last_page_has_no_next() {
    let f = Fixture::new(UsageCursorPolicy::Strict);
    for path in [LIST.to_string(), format!("{ALL}?owner=owner")] {
        let separator = if path.contains('?') { '&' } else { '?' };
        let first = f
            .call(
                &format!("{path}{separator}limit=1"),
                Some("owner"),
                ADMIN,
                200,
            )
            .await;
        assert_eq!(first["items"][0]["sandbox_id"], "sbx_a");
        let token = first["next"].as_str().unwrap();
        assert!(token.starts_with("u1."));
        let next = f
            .call(
                &format!("{path}{separator}limit=2&cursor={token}"),
                Some("owner"),
                ADMIN,
                200,
            )
            .await;
        assert_eq!(
            next["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["sandbox_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["sbx_b", "sbx_c"]
        );
        assert!(next["next"].is_null());
    }
    let events = format!("{LIST}/sbx_a/events");
    let token = f.first(&events, OWN).await;
    let next = f
        .call(
            &format!("{events}?limit=2&cursor={token}"),
            Some("owner"),
            OWN,
            200,
        )
        .await;
    assert_eq!(
        next["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["revision"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert!(next["next"].is_null());
}

#[tokio::test]
async fn usage_tokens_cannot_cross_principal_mode_filter_or_endpoint_before_page_reads() {
    let f = Fixture::new(UsageCursorPolicy::Strict);
    let own = f.first(LIST, OWN).await;
    let admin = f.first(ALL, ADMIN).await;
    let filtered = f.first(&format!("{ALL}?owner=owner"), ADMIN).await;
    let events = f.first(&format!("{LIST}/sbx_a/events"), OWN).await;
    let before = f.effects();
    for (path, subject) in [
        (format!("{LIST}?cursor={own}"), "other"),
        (format!("{ALL}?cursor={own}"), "owner"),
        (format!("{LIST}?cursor={admin}"), "owner"),
        (format!("{ALL}?owner=owner&cursor={admin}"), "owner"),
        (format!("{ALL}?owner=other&cursor={filtered}"), "owner"),
        (format!("{LIST}/sbx_a/events?cursor={own}"), "owner"),
        (format!("{LIST}?cursor={events}"), "owner"),
        (format!("{LIST}/sbx_b/events?cursor={events}"), "owner"),
        (
            format!("{LIST}/sbx_a/events?owner=owner&cursor={events}"),
            "owner",
        ),
    ] {
        f.call(&path, Some(subject), ADMIN, 400).await;
    }
    assert_eq!(f.effects(), before);
}

#[tokio::test]
async fn continuation_is_not_authority_and_event_resource_concealment_precedes_decode() {
    let f = Fixture::new(UsageCursorPolicy::Strict);
    let token = f.first(LIST, OWN).await;
    let before = f.effects();
    f.call(&format!("{LIST}?cursor={token}"), None, OWN, 401)
        .await;
    f.call(&format!("{LIST}?cursor={token}"), Some("owner"), &[], 403)
        .await;
    f.call(&format!("{ALL}?cursor={token}"), Some("owner"), OWN, 403)
        .await;
    for id in ["sbx_d", "missing"] {
        let a = f
            .call(
                &format!("{LIST}/{id}/events?cursor={token}"),
                Some("owner"),
                OWN,
                404,
            )
            .await;
        let b = f
            .call(
                &format!("{LIST}/{id}/events?cursor=u1.invalid"),
                Some("owner"),
                OWN,
                404,
            )
            .await;
        assert_eq!(a, b);
    }
    f.call(
        &format!("{LIST}/sbx_a/events?cursor={token}"),
        Some("owner"),
        &[],
        404,
    )
    .await;
    assert_eq!(f.effects(), before);
}

#[tokio::test]
async fn expired_and_malformed_continuations_are_private_fixed_problems_without_page_reads() {
    let f = Fixture::new(UsageCursorPolicy::Strict);
    let token = f.first(LIST, OWN).await;
    let before = f.effects();
    f.clock.store(1000, Ordering::SeqCst);
    for token in [
        token,
        "invalid".into(),
        "zzz".into(),
        "u1.bad.tag".into(),
        "u2.bad.tag".into(),
        "u1.%FF.tag".into(),
        "x".repeat(4097),
    ] {
        for path in [LIST.to_string(), ALL.into(), format!("{LIST}/sbx_a/events")] {
            f.call(&format!("{path}?cursor={token}"), Some("owner"), ADMIN, 400)
                .await;
        }
    }
    assert_eq!(f.effects(), before);
    assert!(f.store.reads.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn reader_first_transition_retains_legacy_behavior_but_never_accepts_bad_signed_tokens() {
    let f = Fixture::new(UsageCursorPolicy::ReaderOnly);
    let next = f.first(LIST, OWN).await;
    assert_eq!(next, "sbx_a");
    let body = f
        .call(&format!("{LIST}?cursor=invalid"), Some("owner"), OWN, 200)
        .await;
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    let body = f
        .call(&format!("{LIST}?cursor=zzz"), Some("owner"), OWN, 200)
        .await;
    assert!(body["items"].as_array().unwrap().is_empty());
    let before = f.effects();
    f.call(
        &format!("{LIST}?cursor=u1.bad.tag"),
        Some("owner"),
        OWN,
        400,
    )
    .await;
    assert_eq!(f.effects(), before);
}

#[test]
fn usage_openapi_documents_bound_continuations_and_typed_refusals() {
    let document = serde_json::to_value(super::super::document()).unwrap();
    for path in [LIST, ALL, "/api/v1/usage/sandboxes/{id}/events"] {
        let operation = &document["paths"][path]["get"];
        let cursor = operation["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "cursor")
            .unwrap();
        assert_eq!(cursor["in"], "query");
        assert_eq!(cursor["schema"]["maxLength"], 4096);
        let description = cursor["description"].as_str().unwrap();
        assert!(description.contains("15 minutes") && description.contains("same read view"));
        assert!(operation["responses"]["400"]["content"]["application/problem+json"].is_object());
    }
}

#[tokio::test]
async fn snapshot_pages_share_as_of_and_missing_history_returns_the_restart_problem() {
    let fixture = Fixture::with_snapshots(UsageCursorPolicy::Strict, true);
    let first = fixture
        .call(&format!("{LIST}?limit=1"), Some("owner"), OWN, 200)
        .await;
    assert_eq!(first["as_of"], 100);
    let next = first["next"].as_str().unwrap();
    assert!(next.starts_with("u2."));
    fixture.clock.store(120, Ordering::SeqCst);
    let path = format!("{LIST}?limit=1&cursor={next}");
    let second = fixture.call(&path, Some("owner"), OWN, 200).await;
    assert_eq!(second["as_of"], 100);
    fixture.store.snapshot_missing.store(true, Ordering::SeqCst);
    let missing = fixture.call(&path, Some("owner"), OWN, 400).await;
    assert_eq!(missing["code"], "invalid_query");
    assert_eq!(
        missing["detail"],
        "Usage continuation is invalid or expired. Restart from the first page without a cursor."
    );
}
