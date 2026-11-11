use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Peer};
use serde_json::json;

fn source(peer: &Peer) -> CmaProfileAuthoringSource {
    let http = http::client().unwrap();
    let origin = reqwest::Url::parse(&peer.origin).unwrap();
    CmaProfileAuthoringSource {
        issuer: None,
        tokens: None,
        skills: skills::SkillCatalog {
            http: http.clone(),
            origin: origin.clone(),
        },
        repositories: repositories::RepositoryResolver { http, origin },
    }
}

fn search(
    source: CmaProfileAuthoringSource,
) -> tokio::task::JoinHandle<Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError>> {
    tokio::spawn(async move {
        source
            .skills(
                &SubjectId::new("fixture-owner"),
                &SkillCatalogQuery {
                    query: "research".into(),
                    limit: 4,
                    cursor: None,
                },
                None,
            )
            .await
    })
}

#[tokio::test]
async fn catalogue_unavailable_is_typed_without_retry() {
    for status in [401, 429, 500, 503] {
        let mut peer = Peer::start().await;
        let result = search(source(&peer));
        let request = peer.next().await;
        assert_eq!(
            request.uri.path(),
            "/api/v1/proxy/s/ornn-api/api/v1/skill-search"
        );
        request.respond(status, "upstream diagnostic");
        assert!(matches!(
            finish(result).await,
            Err(AuthoringSourceError::Unavailable(_))
        ));
        assert_eq!(peer.count(), 1);
    }
}

#[tokio::test]
async fn stalled_search_and_version_body_expire_without_partial_catalogue() {
    for versions in [false, true] {
        let mut peer = Peer::start().await;
        let result = search(source(&peer));
        let request = peer.next().await;
        let held = if versions {
            request.json(json!({"data":{"items":[{"guid":"research","name":"Research"}],"meta":{"hasMore":false,"nextCursor":null}},"error":null}));
            let request = peer.next().await;
            assert_eq!(
                request.uri.path(),
                "/api/v1/proxy/s/ornn-api/api/v1/skills/research/versions"
            );
            request.stall_body();
            None
        } else {
            Some(request)
        };
        tokio::time::pause();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        tokio::time::advance(Duration::from_secs(15)).await;
        assert!(matches!(
            finish(result).await,
            Err(AuthoringSourceError::Unavailable(_))
        ));
        assert_deadline(deadline);
        tokio::time::advance(Duration::from_secs(120)).await;
        assert_eq!(peer.count(), if versions { 2 } else { 1 });
        drop(held);
        tokio::time::resume();
    }
}

struct PendingToken;

#[async_trait::async_trait]
impl OwnerTokenSource for PendingToken {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn owner_token_wait_is_bounded_by_the_catalogue_deadline() {
    let peer = Peer::start().await;
    let mut source = source(&peer);
    source.tokens = Some(Arc::new(PendingToken));
    tokio::time::pause();
    let result = search(source);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!result.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(
        matches!(finish(result).await, Err(AuthoringSourceError::Unavailable(detail)) if detail == "skill catalogue deadline elapsed")
    );
    assert_eq!(peer.count(), 0);
}
