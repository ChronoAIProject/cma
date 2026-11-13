use super::*;
use layer2_core::sandbox::colors::OwnerSandboxColors;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::{
    DirectoryQuery, PublicIdentity, RegisteredUser, UserDirectory, UserDirectoryStore,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Reads {
    color_started: tokio::sync::Notify,
    name_started: tokio::sync::Notify,
    color_release: tokio::sync::Notify,
    wait: AtomicBool,
    names: AtomicBool,
    fail_colors: bool,
    fail_names: bool,
}

#[async_trait]
impl SandboxColorStore for Reads {
    async fn owner_colors(&self, _: &SubjectId) -> Result<OwnerSandboxColors, SandboxStoreError> {
        self.color_started.notify_one();
        if self.wait.load(Ordering::SeqCst) {
            self.color_release.notified().await;
        }
        if self.fail_colors {
            return Err(SandboxStoreError::Unavailable("fixture unavailable".into()));
        }
        Ok(OwnerSandboxColors {
            revision: 1,
            colors: BTreeMap::from([(
                SandboxId("sbx_1".into()),
                SandboxColor::parse("#c04080").unwrap(),
            )]),
        })
    }
    async fn record(
        &self,
        _: &SubjectId,
        _: u64,
        _: &[(SandboxId, SandboxColor)],
    ) -> Result<(), SandboxStoreError> {
        panic!("a retained palette must not be rewritten");
    }
}

#[async_trait]
impl UserDirectoryStore for Reads {
    async fn observe(&self, _: &PublicIdentity) -> Result<(), StoreError> {
        panic!("read only");
    }
    async fn user(&self, _: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        panic!("use bounded names");
    }
    async fn list(
        &self,
        _: &DirectoryQuery,
    ) -> Result<layer2_core::Page<RegisteredUser>, StoreError> {
        panic!("never enumerate");
    }
    async fn display_names(
        &self,
        subjects: &[SubjectId],
    ) -> Result<BTreeMap<String, String>, StoreError> {
        assert_eq!(subjects, &[SubjectId::new("owner")]);
        self.names.store(true, Ordering::SeqCst);
        self.name_started.notify_one();
        if self.fail_names {
            return Err(StoreError::Unavailable("fixture unavailable".into()));
        }
        Ok(BTreeMap::from([("owner".into(), "Fixture owner".into())]))
    }
}

async fn enriched(reads: Arc<Reads>, concurrent: bool) -> Fixture {
    let mut fixture = fixture(10);
    fixture
        .store
        .create_sandbox(stored_sandbox("sbx_1", "owner", SandboxStatus::Active, 4))
        .await
        .unwrap();
    fixture.state = fixture
        .state
        .with_concurrent_detail_enrichment(concurrent)
        .with_user_directory(UserDirectory::new(reads.clone()))
        .with_sandbox_colors(SandboxColorService::new(
            reads,
            fixture.store.clone(),
            Arc::new(SystemColorRandom::new()),
        ));
    fixture.router = hierarchy_router(fixture.state.clone());
    fixture
}

#[tokio::test]
async fn enrichment_concurrency_preserves_responses_and_independent_failure_fallbacks() {
    for (fail_colors, fail_names) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut expected = None;
        for concurrent in [false, true] {
            let reads = Arc::new(Reads {
                fail_colors,
                fail_names,
                ..Reads::default()
            });
            let fixture = enriched(reads, concurrent).await;
            let (status, body) = json_response(
                &fixture.router,
                authenticated(Method::GET, "/api/v1/sandboxes/sbx_1", None, None),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body.get("color").is_none(), fail_colors);
            // Unlike color, the existing name field serializes an absent value
            // as null. Keep the exact wire shape in both scheduling modes.
            assert_eq!(
                body.get("owner_name"),
                Some(&if fail_names {
                    Value::Null
                } else {
                    Value::String("Fixture owner".into())
                })
            );
            if let Some(expected) = &expected {
                assert_eq!(&body, expected);
            } else {
                expected = Some(body);
            }
        }
    }
}

#[tokio::test]
async fn enrichment_starts_names_while_color_is_pending_only_when_enabled() {
    for concurrent in [false, true] {
        let reads = Arc::new(Reads::default());
        reads.wait.store(true, Ordering::SeqCst);
        let fixture = enriched(reads.clone(), concurrent).await;
        let task = tokio::spawn(async move {
            json_response(
                &fixture.router,
                authenticated(Method::GET, "/api/v1/sandboxes/sbx_1", None, None),
            )
            .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reads.color_started.notified(),
        )
        .await
        .unwrap();
        if concurrent {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                reads.name_started.notified(),
            )
            .await
            .unwrap();
        }
        assert_eq!(reads.names.load(Ordering::SeqCst), concurrent);
        reads.color_release.notify_one();
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["color"], "#c04080");
        assert_eq!(body["owner_name"], "Fixture owner");
    }
}

#[tokio::test]
async fn refused_hierarchy_never_starts_optional_enrichment() {
    for concurrent in [false, true] {
        let reads = Arc::new(Reads::default());
        let fixture = enriched(reads.clone(), concurrent).await;
        let (status, _) = json_response(
            &fixture.router,
            authenticated(Method::GET, "/api/v1/sandboxes/sbx_absent", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!reads.names.load(Ordering::SeqCst));
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, reads.color_started.notified())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn shared_details_never_start_owner_enrichment_in_either_mode() {
    use crate::rest::{resource_access_tests::context, shared_agent_tests::Reply};
    let mut expected = None;
    for concurrent in [false, true] {
        let (mut fixture, _, _, _) =
            crate::rest::shared_hierarchy_tests::fixture(true, Reply::Member).await;
        let reads = Arc::new(Reads::default());
        fixture.state = fixture
            .state
            .with_concurrent_detail_enrichment(concurrent)
            .with_user_directory(UserDirectory::new(reads.clone()))
            .with_sandbox_colors(SandboxColorService::new(
                reads.clone(),
                fixture.store.clone(),
                Arc::new(SystemColorRandom::new()),
            ));
        fixture.router = hierarchy_router(fixture.state);
        let mut request = crate::rest::agents_tests::request(
            Method::GET,
            "/api/v1/sandboxes/sbx_route",
            "recipient",
            None,
            None,
        );
        request.extensions_mut().insert(context("recipient"));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
        let (status, body) = json_response(&fixture.router, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["access"], "shared");
        assert!(!reads.names.load(Ordering::SeqCst));
        assert!(
            tokio::time::timeout(std::time::Duration::ZERO, reads.color_started.notified())
                .await
                .is_err()
        );
        if let Some(expected) = &expected {
            assert_eq!(&body, expected);
        } else {
            expected = Some(body);
        }
    }
}
