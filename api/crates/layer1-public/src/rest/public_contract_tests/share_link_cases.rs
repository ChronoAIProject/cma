use super::{super::agents_tests, Scenario};
use axum::Extension;
use layer2_core::{
    resource_access::*, sandbox::AgentStatus, AgentId, Caller, DirectoryQuery, Page,
    PublicIdentity, RegisteredUser, StoreError, SubjectId, UserDirectory, UserDirectoryStore,
};
use sha2::{Digest as _, Sha256};
use std::{collections::BTreeMap, sync::Arc};

struct Directory;
#[async_trait::async_trait]
impl UserDirectoryStore for Directory {
    async fn observe(&self, _: &PublicIdentity) -> Result<(), StoreError> {
        Ok(())
    }
    async fn user(&self, subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        Ok(["owner-a", "recipient"]
            .contains(&subject.as_str())
            .then(|| RegisteredUser {
                identity: PublicIdentity {
                    subject: subject.clone(),
                    display_name: Some(format!("Name {subject}")),
                    avatar_url: None,
                },
                first_seen_at: "2026-09-05T00:00:00Z".into(),
                last_seen_at: "2026-09-05T00:00:00Z".into(),
            }))
    }
    async fn list(&self, query: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        let mut items = Vec::new();
        for subject in ["owner-a", "recipient"] {
            if query
                .text
                .as_ref()
                .is_none_or(|text| format!("Name {subject}").contains(text))
            {
                items.push(self.user(&SubjectId::new(subject)).await?.unwrap());
            }
        }
        Ok(Page {
            items,
            next_cursor: None,
        })
    }
}

struct Authority;
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("typed_link_") {
        return None;
    }
    let fixture = agents_tests::fixture().await;
    agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_share", AgentStatus::Ready, 0, None),
        Vec::new(),
    )
    .await;
    let target = ResourceTarget::Agent {
        agent_id: AgentId("agt_share".into()),
    };
    let snapshot = fixture
        .store
        .resource_access_snapshot(&target)
        .await
        .unwrap();
    let audience = if name.contains("selected") {
        AgentShareAudience::Users {
            subjects: vec![SubjectId::new("recipient")],
        }
    } else if name.contains("organization") {
        AgentShareAudience::Organization {
            organization_id: SubjectId::new("org"),
        }
    } else {
        AgentShareAudience::Public
    };
    fixture
        .store
        .mutate_share(ShareMutation {
            selector: ResourceSelector::Agent {
                agent_id: AgentId("agt_share".into()),
            },
            owner: ResourceOwner::Person {
                subject: SubjectId::new("owner-a"),
            },
            actor: ResourceActor::Api {
                subject: SubjectId::new("owner-a"),
                delegation_id: None,
            },
            key_sha256: format!("{:x}", Sha256::digest("fixture-typed-link")),
            change: ShareChange::CreateLink {
                audience,
                mode: ShareMode::View,
                token_sha256: format!("{:x}", Sha256::digest("a".repeat(64))),
            },
            expected_share_revision: None,
            policy_fence: snapshot.fence(),
            proposed_id: "shr_link_fixture".into(),
            at: "2026-09-05T00:00:00Z".into(),
        })
        .await
        .unwrap();
    let subject = if name.contains("recipient") {
        "recipient"
    } else if name.contains("outsider") {
        "outsider"
    } else {
        "owner-a"
    };
    let context = super::super::resource_access_tests::context(subject);
    let service = ResourceAccessService::new(fixture.store, Arc::new(Authority));
    let service = if name.contains("directory_unavailable") {
        service
    } else {
        service.with_directory(UserDirectory::new(Arc::new(Directory)))
    };
    let router = super::super::resource_access_router(Arc::new(service))
        .layer(Extension(context.clone()))
        .layer(Extension(crate::DelegatedCredential::new(
            "request-credential".into(),
        )));
    Some(Scenario {
        router,
        subject: (!name.contains("anonymous")).then_some(subject),
        authorization: context.caller.authorization,
        symbols: BTreeMap::new(),
    })
}
