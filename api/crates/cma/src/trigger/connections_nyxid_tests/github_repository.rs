use super::*;
use crate::trigger::GitHubRepositoryNeed::PublicRepositoryWrite;

pub(crate) struct GithubRepositoryFixture {
    provider: Provider,
}

impl GithubRepositoryFixture {
    pub(crate) async fn start() -> Self {
        let provider = Provider::start().await;
        *provider.state.catalog_reply.lock().unwrap() =
            Reply::json(json!({"services":[{"id":"github-catalog","slug":"api-github"}]}));
        let row = |id, slug| {
            json!({
                "id":id, "slug":slug, "catalog_service_id":"github-catalog", "is_active":true,
                "credential_source":{"type":"personal"}
            })
        };
        let key = |id, slug| {
            Reply::json(json!({
                "id":id, "slug":slug, "label":id, "catalog_service_id":"github-catalog",
                "is_active":true, "status":"active", "connection_status":"active",
                "credential_type":"oauth2", "granted_scopes":["public_repo"]
            }))
        };
        provider.state.set_services_reply(Reply::json(json!({
            "services":[row("sibling", "api-github"), row("selected", "api-github-2")]
        })));
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("selected".into(), key("selected", "api-github-2"));
        *provider.state.github_proxy_scopes.lock().unwrap() = Some("public_repo".into());
        Self { provider }
    }

    pub(crate) fn set_default_private_read(&self, granted: bool) {
        let scope = if granted { "repo" } else { "public_repo" };
        self.provider.state.set_services_reply(Reply::json(json!({
            "services":[{
                "id":"selected", "slug":"api-github", "catalog_service_id":"github-catalog",
                "is_active":true, "credential_source":{"type":"personal"}
            }]
        })));
        self.provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert(
                "selected".into(),
                Reply::json(json!({
                    "id":"selected", "slug":"api-github", "label":"selected",
                    "catalog_service_id":"github-catalog", "is_active":true,
                    "status":"active", "connection_status":"active", "credential_type":"oauth2",
                    "granted_scopes":[scope]
                })),
            );
        *self.provider.state.github_proxy_scopes.lock().unwrap() = Some(scope.into());
    }

    pub(crate) fn broker(&self) -> Arc<crate::trigger::CmaServiceConnectionBroker> {
        Arc::new(
            crate::trigger::CmaServiceConnectionBroker::with_fixture_provider(broker(
                &self.provider,
                Tokens::available(),
            )),
        )
    }
}

#[tokio::test]
async fn managed_repository_preserves_the_exact_key_and_pinned_commit_on_replay() {
    use crate::trigger::GitHubRepositoryCheck;
    use layer1_public::rest::AgentLifecycleRepository;

    let fixture = GithubRepositoryFixture::start().await;
    let provider = &fixture.provider;
    let broker = fixture.broker();
    let owner = SubjectId::new("visitor");
    let mut request = AgentLifecycleRepository {
        url: "https://github.com/example/project.git".into(),
        reference: Some("branch/synthetic".into()),
        operation: Some(PublicRepositoryWrite),
        connection_id: Some("selected".into()),
    };
    let GitHubRepositoryCheck::Verified(proof) = broker
        .verify_github_repository(&owner, &request, None, None)
        .await
    else {
        panic!("public_repo for the selected writable repository must verify");
    };
    assert_eq!(proof.connection_id, "selected");
    assert_eq!(proof.repository, "example/project");
    assert_eq!(proof.commit, "a".repeat(40));
    request.connection_id = None;
    request.reference = Some("gone".into());
    assert!(
        matches!(broker.verify_github_repository(&owner,&request,Some("selected"),Some(&proof.commit)).await,
        GitHubRepositoryCheck::Verified(replay) if replay.commit == proof.commit && replay.connection_id == proof.connection_id)
    );
    assert!(matches!(
        broker
            .verify_github_repository(&owner, &request, Some("selected"), None)
            .await,
        GitHubRepositoryCheck::ReferenceUnavailable
    ));
    request.url = "https://github.com/example/restricted".into();
    assert!(matches!(
        broker
            .verify_github_repository(&owner, &request, Some("selected"), None)
            .await,
        GitHubRepositoryCheck::RepositoryUnavailable
    ));
    *provider.state.github_proxy_scopes.lock().unwrap() = Some("read:user".into());
    assert!(matches!(
        broker
            .verify_github_repository(&owner, &request, Some("selected"), None)
            .await,
        GitHubRepositoryCheck::Scope(crate::trigger::GitHubScopeCheck::VerificationUnavailable)
    ));
    assert!(provider
        .state
        .github_proxy_reads
        .lock()
        .unwrap()
        .iter()
        .all(|(_, id)| id == "selected"));
}
