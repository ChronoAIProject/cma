use super::*;
use crate::Authorization;
use std::sync::Mutex;

#[derive(Default)]
struct Catalog {
    calls: Mutex<Vec<(String, String)>>,
}

impl Catalog {
    fn record(&self, owner: &SubjectId, credential: Option<&AuthoringCredential>) {
        self.calls
            .lock()
            .unwrap()
            .push((owner.to_string(), credential.unwrap().value().to_owned()));
    }
}

#[async_trait]
impl ProfileAuthoringSource for Catalog {
    async fn services(
        &self,
        owner: &SubjectId,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Vec<AuthoringServiceEntry>, AuthoringSourceError> {
        self.record(owner, credential);
        Ok(vec![AuthoringServiceEntry {
            id: ServiceId::new(format!("private-{owner}")),
            catalog_id: None,
            name: "Private service".into(),
            description: None,
        }])
    }

    async fn skills(
        &self,
        owner: &SubjectId,
        _: &SkillCatalogQuery,
        credential: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError> {
        self.record(owner, credential);
        Ok(ProfilePage {
            items: vec![AuthoringSkillEntry {
                id: SkillId(format!("private-{owner}")),
                name: "Private skill".into(),
                description: None,
                versions: vec!["1.0".into()],
            }],
            next: None,
        })
    }

    async fn resolve_repository(
        &self,
        _: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        panic!("catalog permission must not authorize repository resolution")
    }
}

fn query() -> SkillCatalogQuery {
    SkillCatalogQuery {
        query: String::new(),
        limit: 20,
        cursor: None,
    }
}

fn service(source: Arc<Catalog>) -> Arc<ProfileAuthoringService> {
    ProfileAuthoringService::new(
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        source,
    )
}

#[tokio::test]
async fn sandbox_users_and_profile_managers_read_only_their_own_catalogs() {
    let source = Arc::new(Catalog::default());
    let service = service(source.clone());
    for capabilities in [
        vec![Capability::UseOwnSandboxes],
        vec![Capability::ManageOwnProfiles],
        vec![Capability::UseOwnSandboxes, Capability::ManageOwnProfiles],
    ] {
        for owner in ["owner-a", "owner-b"] {
            let caller = Caller {
                subject: SubjectId::new(owner),
                authorization: Authorization::new(capabilities.clone()),
            };
            let credential = AuthoringCredential::new(format!("synthetic-authority-{owner}"));
            let services = service.services(&caller, Some(&credential)).await.unwrap();
            let skills = service
                .skills(&caller, query(), Some(&credential))
                .await
                .unwrap();
            assert_eq!(services[0].id.as_str(), format!("private-{owner}"));
            assert_eq!(skills.items[0].id.0, format!("private-{owner}"));
        }
    }
    let calls = source.calls.lock().unwrap();
    assert_eq!(calls.len(), 12);
    for (owner, credential) in calls.iter() {
        assert_eq!(credential, &format!("synthetic-authority-{owner}"));
    }
}

#[tokio::test]
async fn catalog_access_never_grants_profile_management_or_calls_sources_without_permission() {
    let source = Arc::new(Catalog::default());
    let service = service(source.clone());
    let mut caller = Caller {
        subject: SubjectId::new("owner-a"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    assert_eq!(
        service.authorize(&caller),
        Err(ProfileAuthoringError::Forbidden)
    );
    assert_eq!(
        service.bounds(&caller),
        Err(ProfileAuthoringError::Forbidden)
    );
    assert_eq!(
        service
            .resolve_repository(
                &caller,
                RepositoryRequest {
                    url: "https://github.com/example/research".into(),
                    reference: None,
                }
            )
            .await,
        Err(ProfileAuthoringError::Forbidden)
    );
    for capabilities in [vec![], vec![Capability::CreateSandbox]] {
        caller.authorization = Authorization::new(capabilities);
        assert!(service.services(&caller, None).await.is_err());
        assert!(service.skills(&caller, query(), None).await.is_err());
    }
    assert!(source.calls.lock().unwrap().is_empty());
}
