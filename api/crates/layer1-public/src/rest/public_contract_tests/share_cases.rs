use super::{super::agents_tests, Scenario};
use axum::Extension;
use layer2_core::resource_access::*;
use layer2_core::sandbox::AgentStatus;
use layer2_core::{Caller, SubjectId};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

struct Authority {
    outage: bool,
}
fn context(subject: &str) -> CallerContext {
    super::super::resource_access_tests::context(subject)
}
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        org: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if self.outage {
            return Err(MembershipError::Unavailable);
        }
        if caller.subject.as_str() == "outsider" {
            return Err(MembershipError::NotMember);
        }
        let mut context = context(caller.subject.as_str());
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            org.clone(),
            OrganizationRole::Admin,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(context)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}
struct UnavailableStore;
#[async_trait::async_trait]
impl ResourceAccessStore for UnavailableStore {
    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        Err(ResourceAccessStoreError::Unavailable)
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for UnavailableStore {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        unreachable!()
    }
}
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !matches!(
        name,
        "share_anonymous"
            | "share_capacity"
            | "share_effective"
            | "share_empty_create"
            | "share_empty_outage"
            | "share_empty_unsafe"
            | "share_list"
            | "share_outsider"
            | "share_owner"
            | "share_read"
            | "share_recipient"
            | "share_recipient_effective"
            | "share_recipient_outage"
            | "share_revoke"
            | "share_stale_update"
            | "share_store_unavailable"
            | "share_update"
    ) {
        return None;
    }
    let fixture = agents_tests::fixture().await;
    agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_share", AgentStatus::Ready, 0, None),
        Vec::new(),
    )
    .await;
    let subject = if name.contains("outsider") {
        "outsider"
    } else if name.contains("recipient") {
        "recipient"
    } else {
        "owner-a"
    };
    let context = context(subject);
    let target = ResourceTarget::Agent {
        agent_id: layer2_core::AgentId("agt_share".into()),
    };
    if !name.contains("empty") {
        use sha2::{Digest as _, Sha256};
        let count = if name.contains("capacity") { 128 } else { 1 };
        for index in 0..count {
            let snapshot = fixture
                .store
                .resource_access_snapshot(&target)
                .await
                .unwrap();
            fixture
                .store
                .mutate_share(ShareMutation {
                    selector: ResourceSelector::Agent {
                        agent_id: layer2_core::AgentId("agt_share".into()),
                    },
                    owner: ResourceOwner::Person {
                        subject: SubjectId::new("owner-a"),
                    },
                    actor: ResourceActor::Api {
                        subject: SubjectId::new("owner-a"),
                        delegation_id: None,
                    },
                    key_sha256: format!("{:x}", Sha256::digest(format!("fixture-{index}"))),
                    change: ShareChange::Create {
                        target_org_id: SubjectId::new("org"),
                        mode: ShareMode::View,
                    },
                    expected_share_revision: None,
                    policy_fence: snapshot.fence(),
                    proposed_id: if index == 0 {
                        "shr_fixture".into()
                    } else {
                        format!("shr_fixture_{index}")
                    },
                    at: "2026-09-05T00:00:00Z".into(),
                })
                .await
                .unwrap();
        }
    }
    let store: Arc<dyn ResourceShareStore> = if name.contains("store_unavailable") {
        Arc::new(UnavailableStore)
    } else {
        fixture.store
    };
    let service = ResourceAccessService::new(
        store,
        Arc::new(Authority {
            outage: name.contains("outage"),
        }),
    );
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
