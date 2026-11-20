use std::sync::Mutex;

use super::*;
use crate::Authorization;

#[derive(Clone)]
struct Source {
    calls: Arc<Mutex<Vec<String>>>,
    expected_credential: Option<String>,
    services: Result<Vec<AuthoringServiceEntry>, AuthoringSourceError>,
    skills: Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError>,
    repository: Result<RepositoryResolution, AuthoringSourceError>,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            calls: Default::default(),
            expected_credential: None,
            services: Ok(vec![AuthoringServiceEntry {
                id: ServiceId::new("research"),
                catalog_id: None,
                name: "Research".into(),
                description: Some("Owner-visible research service".into()),
            }]),
            skills: Ok(ProfilePage {
                items: vec![AuthoringSkillEntry {
                    id: SkillId("research".into()),
                    name: "Research".into(),
                    description: None,
                    versions: vec!["10.2".into(), "2.10".into(), "2.9".into()],
                }],
                next: None,
            }),
            repository: Ok(RepositoryResolution {
                url: "https://github.com/Example/Research".into(),
                reference: None,
                default_branch: "main".into(),
                commit_sha: "a".repeat(40),
            }),
        }
    }
}

#[async_trait]
impl ProfileAuthoringSource for Source {
    async fn services(
        &self,
        owner: &SubjectId,
        credential: Option<&AuthoringCredential>,
    ) -> Result<Vec<AuthoringServiceEntry>, AuthoringSourceError> {
        assert_eq!(
            credential.map(AuthoringCredential::value),
            self.expected_credential.as_deref()
        );
        self.calls.lock().unwrap().push(format!("services:{owner}"));
        self.services.clone()
    }

    async fn skills(
        &self,
        owner: &SubjectId,
        _: &SkillCatalogQuery,
        credential: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError> {
        assert_eq!(
            credential.map(AuthoringCredential::value),
            self.expected_credential.as_deref()
        );
        self.calls.lock().unwrap().push(format!("skills:{owner}"));
        self.skills.clone()
    }

    async fn resolve_repository(
        &self,
        _: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        self.calls.lock().unwrap().push("repository".into());
        self.repository.clone()
    }
}

fn owner() -> Caller {
    Caller {
        subject: SubjectId::new("owner-a"),
        authorization: Authorization::new([Capability::ManageOwnProfiles]),
    }
}

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8000,
        min_memory_mib: 128,
        max_memory_mib: 16384,
    }
}

fn service(source: Source) -> Arc<ProfileAuthoringService> {
    ProfileAuthoringService::new(bounds(), Arc::new(source))
}

fn query() -> SkillCatalogQuery {
    SkillCatalogQuery {
        query: String::new(),
        limit: DEFAULT_SKILL_PAGE_SIZE,
        cursor: None,
    }
}

fn repository() -> RepositoryRequest {
    RepositoryRequest {
        url: "https://github.com/example/research.git/".into(),
        reference: None,
    }
}

#[tokio::test]
async fn authoring_requires_permission_before_validation_or_source_reads() {
    let source = Source::default();
    let calls = source.calls.clone();
    let service = service(source);
    let mut caller = owner();
    caller.authorization = Authorization::none();
    assert_eq!(
        service.bounds(&caller),
        Err(ProfileAuthoringError::Forbidden)
    );
    assert_eq!(
        service.services(&caller, None).await,
        Err(ProfileAuthoringError::CatalogForbidden)
    );
    let mut invalid = query();
    invalid.limit = 0;
    assert_eq!(
        service.skills(&caller, invalid, None).await,
        Err(ProfileAuthoringError::CatalogForbidden)
    );
    assert_eq!(
        service
            .resolve_repository(
                &caller,
                RepositoryRequest {
                    url: String::new(),
                    reference: None,
                }
            )
            .await,
        Err(ProfileAuthoringError::Forbidden)
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bounds_are_supplied_policy_and_catalog_reads_are_owner_scoped() {
    let mut source = Source::default();
    source
        .services
        .as_mut()
        .unwrap()
        .push(AuthoringServiceEntry {
            id: ServiceId::new("analytics"),
            catalog_id: None,
            name: "Analytics".into(),
            description: None,
        });
    let calls = source.calls.clone();
    let service = service(source);
    assert_eq!(service.bounds(&owner()).unwrap(), bounds());
    assert!(calls.lock().unwrap().is_empty());
    let services = service.services(&owner(), None).await.unwrap();
    assert_eq!(services[0].id.as_str(), "analytics");
    let skills = service.skills(&owner(), query(), None).await.unwrap();
    assert_eq!(skills.items[0].versions, ["10.2", "2.10", "2.9"]);
    assert_eq!(
        *calls.lock().unwrap(),
        ["services:owner-a", "skills:owner-a"]
    );
}

#[test]
#[should_panic(expected = "valid deployment capacity bounds")]
fn invalid_deployment_bounds_fail_at_composition() {
    let mut invalid = bounds();
    invalid.min_cpu_millis = 0;
    ProfileAuthoringService::new(invalid, Arc::new(Source::default()));
}

#[tokio::test]
async fn transient_lookup_authority_is_redacted_and_forwarded_only_after_authorization() {
    let credential = AuthoringCredential::new("test-request-authority".into());
    assert_eq!(format!("{credential:?}"), "AuthoringCredential(<redacted>)");
    let source = Source {
        expected_credential: Some(credential.value().into()),
        ..Source::default()
    };
    let calls = source.calls.clone();
    let service = service(source);
    let mut forbidden = owner();
    forbidden.authorization = Authorization::none();
    assert_eq!(
        service.services(&forbidden, Some(&credential)).await,
        Err(ProfileAuthoringError::CatalogForbidden)
    );
    assert_eq!(
        service.skills(&forbidden, query(), Some(&credential)).await,
        Err(ProfileAuthoringError::CatalogForbidden)
    );
    assert!(calls.lock().unwrap().is_empty());
    service.services(&owner(), Some(&credential)).await.unwrap();
    service
        .skills(&owner(), query(), Some(&credential))
        .await
        .unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        ["services:owner-a", "skills:owner-a"]
    );
}

#[tokio::test]
async fn invalid_skill_queries_never_reach_the_source() {
    let source = Source::default();
    let calls = source.calls.clone();
    let service = service(source);
    for invalid in [
        SkillCatalogQuery {
            limit: 0,
            ..query()
        },
        SkillCatalogQuery {
            limit: 51,
            ..query()
        },
        SkillCatalogQuery {
            query: "x".repeat(201),
            ..query()
        },
        SkillCatalogQuery {
            query: "line\nbreak".into(),
            ..query()
        },
        SkillCatalogQuery {
            cursor: Some(String::new()),
            ..query()
        },
        SkillCatalogQuery {
            cursor: Some("x".repeat(2049)),
            ..query()
        },
    ] {
        assert_eq!(
            service.skills(&owner(), invalid, None).await,
            Err(ProfileAuthoringError::InvalidQuery)
        );
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn source_failures_keep_operation_specific_refusals_and_hide_details() {
    for failure in [
        AuthoringSourceError::Unavailable("private diagnostics".into()),
        AuthoringSourceError::InvalidResponse("private diagnostics".into()),
    ] {
        let service = service(Source {
            services: Err(failure.clone()),
            skills: Err(failure.clone()),
            repository: Err(failure),
            ..Source::default()
        });
        assert_eq!(
            service.services(&owner(), None).await,
            Err(ProfileAuthoringError::ServicesUnavailable)
        );
        assert_eq!(
            service.skills(&owner(), query(), None).await,
            Err(ProfileAuthoringError::SkillsUnavailable)
        );
        assert_eq!(
            service.resolve_repository(&owner(), repository()).await,
            Err(ProfileAuthoringError::RepositoryUnavailable)
        );
    }
    let service = service(Source {
        skills: Err(AuthoringSourceError::InvalidQuery(
            "opaque cursor invalid".into(),
        )),
        ..Source::default()
    });
    assert_eq!(
        service.skills(&owner(), query(), None).await,
        Err(ProfileAuthoringError::InvalidQuery)
    );
}

#[tokio::test]
async fn skill_pages_reject_duplicate_identity_nonadvancing_cursor_and_invalid_versions() {
    let valid = Source::default().skills.unwrap();
    let mut invalid_pages = Vec::new();
    let mut duplicate = valid.clone();
    duplicate.items.push(duplicate.items[0].clone());
    invalid_pages.push(duplicate);
    for versions in [
        vec![],
        vec!["latest"],
        vec!["01.2"],
        vec!["1.2.3"],
        vec!["2.9", "2.10"],
        vec!["2.9", "2.9"],
    ] {
        let mut invalid = valid.clone();
        invalid.items[0].versions = versions.into_iter().map(str::to_owned).collect();
        invalid_pages.push(invalid);
    }
    let mut overfull = valid.clone();
    overfull.items = vec![valid.items[0].clone(); 21];
    invalid_pages.push(overfull);
    for page in invalid_pages {
        let service = service(Source {
            skills: Ok(page),
            ..Source::default()
        });
        assert_eq!(
            service.skills(&owner(), query(), None).await,
            Err(ProfileAuthoringError::SkillsUnavailable)
        );
    }
    let service = service(Source {
        skills: Ok(ProfilePage {
            next: Some("same".into()),
            ..valid
        }),
        ..Source::default()
    });
    assert_eq!(
        service
            .skills(
                &owner(),
                SkillCatalogQuery {
                    cursor: Some("same".into()),
                    ..query()
                },
                None,
            )
            .await,
        Err(ProfileAuthoringError::SkillsUnavailable)
    );
}

#[tokio::test]
async fn filtered_empty_skill_pages_may_advance_to_more_results() {
    let expected = ProfilePage {
        items: vec![],
        next: Some("later-page".into()),
    };
    let service = service(Source {
        skills: Ok(expected.clone()),
        ..Source::default()
    });
    assert_eq!(
        service.skills(&owner(), query(), None).await.unwrap(),
        expected
    );
}

#[tokio::test]
async fn malformed_catalog_rows_fail_closed() {
    let valid = Source::default().services.unwrap();
    let mut variants = vec![vec![valid[0].clone(), valid[0].clone()]];
    for (name, description) in [
        (String::new(), None),
        ("x".repeat(257), None),
        ("Research".into(), Some("x".repeat(6145))),
    ] {
        variants.push(vec![AuthoringServiceEntry {
            name,
            description,
            ..valid[0].clone()
        }]);
    }
    for entries in variants {
        assert_eq!(
            service(Source {
                services: Ok(entries),
                ..Source::default()
            })
            .services(&owner(), None)
            .await,
            Err(ProfileAuthoringError::ServicesUnavailable)
        );
    }
}

#[tokio::test]
async fn multilingual_catalog_descriptions_keep_the_complete_source_text() {
    let description = "\u{4e00}".repeat(1536);
    let mut source = Source::default();
    source.services.as_mut().unwrap()[0].description = Some(description.clone());
    source.skills.as_mut().unwrap().items[0].description = Some(description.clone());
    let service = service(source);
    assert_eq!(
        service.services(&owner(), None).await.unwrap()[0]
            .description
            .as_deref(),
        Some(description.as_str())
    );
    assert_eq!(
        service.skills(&owner(), query(), None).await.unwrap().items[0]
            .description
            .as_deref(),
        Some(description.as_str())
    );
}

#[tokio::test]
async fn repository_verification_preserves_selection_and_checks_observed_identity() {
    let authoring = service(Source::default());
    let resolved = authoring
        .resolve_repository(&owner(), repository())
        .await
        .unwrap();
    assert_eq!(resolved.url, "https://github.com/Example/Research");
    assert_eq!(resolved.reference, None);
    assert_eq!(resolved.default_branch, "main");
    for reference in ["release/next", "v1.0"] {
        let mut source = Source::default();
        source.repository.as_mut().unwrap().reference = Some(reference.into());
        let verified = service(source)
            .resolve_repository(
                &owner(),
                RepositoryRequest {
                    reference: Some(reference.into()),
                    ..repository()
                },
            )
            .await
            .unwrap();
        assert_eq!(verified.reference.as_deref(), Some(reference));
    }
}

#[tokio::test]
async fn repository_component_bounds_ignore_the_optional_clone_suffix() {
    let canonical = format!("https://github.com/{}/{}", "a".repeat(100), "b".repeat(100));
    let service = service(Source {
        repository: Ok(RepositoryResolution {
            url: canonical.clone(),
            ..Source::default().repository.unwrap()
        }),
        ..Source::default()
    });
    for suffix in ["", "/", ".git", ".git/"] {
        let result = service
            .resolve_repository(
                &owner(),
                RepositoryRequest {
                    url: format!("{canonical}{suffix}"),
                    reference: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(result.url, canonical);
    }
}

#[tokio::test]
async fn invalid_repository_inputs_fail_before_network_resolution() {
    let source = Source::default();
    let calls = source.calls.clone();
    let service = service(source);
    for url in [
        "http://github.com/example/research",
        "https://github.com:443/example/research",
        "https://user@github.com/example/research",
        "https://elsewhere.example/example/research",
        "https://github.com/example/research?token=value",
        "https://github.com/example/%72esearch",
        "https://github.com/example/.git",
        "https://github.com/example/..git",
        "https://github.com/example/...git",
        "https://github.com//example/research",
        "https://github.com/example//research",
        "https://github.com/example/research//",
        "https://github.com/example/./research",
        "https://github.com/example/other/../research",
        "https://github.com/example/research/extra",
    ] {
        assert!(matches!(
            service
                .resolve_repository(
                    &owner(),
                    RepositoryRequest {
                        url: url.into(),
                        reference: None
                    }
                )
                .await,
            Err(ProfileAuthoringError::InvalidRepository(_))
        ));
    }
    for reference in ["", "-option", "line\nbreak"] {
        assert!(matches!(
            service
                .resolve_repository(
                    &owner(),
                    RepositoryRequest {
                        reference: Some(reference.into()),
                        ..repository()
                    }
                )
                .await,
            Err(ProfileAuthoringError::InvalidRepository(_))
        ));
    }
    for url in [
        format!("https://github.com/{}/research", "a".repeat(101)),
        format!("https://github.com/example/{}", "a".repeat(101)),
    ] {
        assert!(matches!(
            service
                .resolve_repository(
                    &owner(),
                    RepositoryRequest {
                        url,
                        reference: None
                    }
                )
                .await,
            Err(ProfileAuthoringError::InvalidRepository(_))
        ));
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn repository_refusals_are_distinct_and_untrusted_output_is_rejected() {
    for (failure, expected) in [
        (
            AuthoringSourceError::RepositoryNotFound,
            ProfileAuthoringError::RepositoryNotFound,
        ),
        (
            AuthoringSourceError::ReferenceNotFound,
            ProfileAuthoringError::ReferenceNotFound,
        ),
    ] {
        assert_eq!(
            service(Source {
                repository: Err(failure),
                ..Source::default()
            })
            .resolve_repository(&owner(), repository())
            .await,
            Err(expected)
        );
    }
    let valid = Source::default().repository.unwrap();
    for invalid in [
        RepositoryResolution {
            url: "https://github.com/foreign/research".into(),
            ..valid.clone()
        },
        RepositoryResolution {
            reference: Some("unrequested".into()),
            ..valid.clone()
        },
        RepositoryResolution {
            default_branch: String::new(),
            ..valid.clone()
        },
        RepositoryResolution {
            commit_sha: "A".repeat(40),
            ..valid.clone()
        },
        RepositoryResolution {
            commit_sha: "a".repeat(39),
            ..valid
        },
    ] {
        assert_eq!(
            service(Source {
                repository: Ok(invalid),
                ..Source::default()
            })
            .resolve_repository(&owner(), repository())
            .await,
            Err(ProfileAuthoringError::RepositoryUnavailable)
        );
    }
}
