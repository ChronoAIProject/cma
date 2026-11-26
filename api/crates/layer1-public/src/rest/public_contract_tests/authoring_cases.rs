use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::profiles::authoring::{
    AuthoringCredential, AuthoringServiceEntry, AuthoringSkillEntry, AuthoringSourceError,
    ProfileAuthoringService, ProfileAuthoringSource, RepositoryRequest, RepositoryResolution,
    SkillCatalogQuery,
};
use layer2_core::profiles::{ProfileCapacityBounds, ProfilePage};
use layer2_core::{Authorization, Capability, ServiceId, SkillId, SubjectId};

use super::Scenario;

struct Source {
    mode: String,
}

impl Source {
    fn check(&self) -> Result<(), AuthoringSourceError> {
        assert!(!self.mode.ends_with("anonymous") && !self.mode.ends_with("forbidden"));
        match self.mode.as_str() {
            "unavailable" => Err(AuthoringSourceError::Unavailable(
                "private upstream diagnostics".into(),
            )),
            "invalid_data" => Err(AuthoringSourceError::InvalidResponse(
                "private upstream diagnostics".into(),
            )),
            _ => Ok(()),
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
        self.check()?;
        assert_eq!(
            credential.map(AuthoringCredential::value),
            (self.mode == "credential_owner" || self.mode == "credential_sandbox_owner")
                .then_some("test-request-authority")
        );
        assert_eq!(owner.as_str(), "contract-owner");
        if self.mode == "empty" {
            return Ok(vec![]);
        }
        let mut entries = vec![
            AuthoringServiceEntry {
                id: ServiceId::new("research"),
                catalog_id: None,
                name: "Research".into(),
                description: Some("Research service".into()),
            },
            AuthoringServiceEntry {
                id: ServiceId::new("analytics"),
                catalog_id: None,
                name: "Analytics".into(),
                description: None,
            },
        ];
        if self.mode == "multilingual" {
            entries[0].description = Some("\u{4e00}".repeat(1536));
        }
        if self.mode == "malformed_data" {
            entries[0].name.clear();
        }
        Ok(entries)
    }

    async fn skills(
        &self,
        owner: &SubjectId,
        query: &SkillCatalogQuery,
        credential: Option<&AuthoringCredential>,
    ) -> Result<ProfilePage<AuthoringSkillEntry>, AuthoringSourceError> {
        self.check()?;
        assert_eq!(
            credential.map(AuthoringCredential::value),
            (self.mode == "credential_owner" || self.mode == "credential_sandbox_owner")
                .then_some("test-request-authority")
        );
        assert_eq!(owner.as_str(), "contract-owner");
        if query.cursor.as_deref() == Some("invalid") {
            return Err(AuthoringSourceError::InvalidQuery("expired cursor".into()));
        }
        if self.mode == "empty" || query.query == "no matches" {
            return Ok(ProfilePage {
                items: vec![],
                next: None,
            });
        }
        if query.query == "filtered" {
            return Ok(ProfilePage {
                items: vec![],
                next: Some("next-filtered".into()),
            });
        }
        let next = if query.cursor.as_deref() == Some("next-page") {
            assert_eq!(query.query, "research");
            assert_eq!(query.limit, 1);
            None
        } else if query.query == "research" {
            assert_eq!(query.limit, 1);
            Some("next-page".into())
        } else {
            assert_eq!(query.query, "");
            assert_eq!(query.limit, 20);
            None
        };
        let mut page = ProfilePage {
            items: vec![AuthoringSkillEntry {
                id: SkillId("research".into()),
                name: "Research".into(),
                description: None,
                versions: vec!["10.2".into(), "2.10".into(), "2.9".into()],
            }],
            next,
        };
        if self.mode == "multilingual" {
            page.items[0].description = Some("\u{4e00}".repeat(1536));
        }
        if self.mode == "malformed_data" {
            page.items[0].versions.reverse();
        }
        Ok(page)
    }

    async fn resolve_repository(
        &self,
        request: &RepositoryRequest,
    ) -> Result<RepositoryResolution, AuthoringSourceError> {
        self.check()?;
        if self.mode == "repository_missing" {
            return Err(AuthoringSourceError::RepositoryNotFound);
        }
        if self.mode == "reference_missing" {
            return Err(AuthoringSourceError::ReferenceNotFound);
        }
        assert_eq!(request.url, "https://github.com/example/research.git/");
        Ok(RepositoryResolution {
            url: if self.mode == "malformed_data" {
                "https://github.com/Foreign/Research"
            } else {
                "https://github.com/Example/Research"
            }
            .into(),
            reference: request.reference.clone(),
            default_branch: "main".into(),
            commit_sha: "a".repeat(40),
        })
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("authoring_")?;
    assert!(matches!(
        mode,
        "owner"
            | "sandbox_owner"
            | "credential_sandbox_owner"
            | "anonymous"
            | "forbidden"
            | "unavailable"
            | "invalid_data"
            | "malformed_data"
            | "multilingual"
            | "credential_owner"
            | "credential_forbidden"
            | "credential_anonymous"
            | "empty"
            | "repository_missing"
            | "reference_missing"
    ));
    let service = ProfileAuthoringService::new(
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        Arc::new(Source { mode: mode.into() }),
    );
    let mut router = super::super::profile_authoring_router(service, None);
    if mode.starts_with("credential_") {
        router = router.layer(axum::Extension(crate::DelegatedCredential::new(
            "test-request-authority".into(),
        )));
    }
    Some(Scenario {
        router,
        subject: (!mode.ends_with("anonymous")).then_some("contract-owner"),
        authorization: Authorization::new(if mode.ends_with("forbidden") {
            vec![]
        } else if mode.ends_with("sandbox_owner") {
            vec![Capability::UseOwnSandboxes]
        } else {
            vec![Capability::ManageOwnProfiles]
        }),
        symbols: Default::default(),
    })
}
