use super::*;
use crate::sandbox::agent_artifacts::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

fn body(name: &str) -> PublishAgentArtifact {
    PublishAgentArtifact {
        name: name.into(),
        media_type: "text/plain".into(),
        content_base64: STANDARD.encode("approved report\n"),
    }
}
fn page() -> ArtifactPageRequest {
    ArtifactPageRequest {
        limit: 1,
        cursor: None,
    }
}

#[tokio::test]
async fn approved_artifacts_are_immutable_narrow_and_preserve_retained_resources() {
    let (store, authority, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let access = Arc::new(access);
    let service = AgentArtifactService::new(store.clone(), access.clone());
    let owner = context("owner");
    let recipient = context("recipient");
    let id = AgentId("agent".into());
    let before = store
        .resource_access_snapshot(&selector.target())
        .await
        .unwrap();
    let logs = store.agent_log(&id, 0, 200).await.unwrap();
    let first = service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &id,
            "one",
            body("report.txt"),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "one",
                body("report.txt")
            )
            .await
            .unwrap(),
        first
    );
    assert!(matches!(
        service
            .publish(
                &owner.caller,
                Some(&owner),
                None,
                &id,
                "one",
                body("different.txt")
            )
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::IdempotencyConflict
        ))
    ));
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &first.id
        )
        .await
        .is_err());
    let share = access
        .change_share(
            &owner,
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap()
        .share;
    let decision = access
        .authorize(
            &recipient,
            Some("request-credential"),
            &selector.target(),
            ResourceCapability::ViewArtifacts,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    assert!(!decision.decision.allows(ResourceCapability::ViewFiles));
    assert!(matches!(
        service
            .publish(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                "recipient",
                body("intrusion.txt")
            )
            .await,
        Err(ResourceAccessError::Access(AccessError::Forbidden))
    ));
    let second = service
        .publish(
            &owner.caller,
            None,
            None,
            &id,
            "legacy",
            body("older caller.txt"),
        )
        .await
        .unwrap();
    let one = service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &page(),
        )
        .await
        .unwrap();
    assert_eq!(one.items.len(), 1);
    let two = service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &ArtifactPageRequest {
                cursor: one.next.clone(),
                ..page()
            },
        )
        .await
        .unwrap();
    assert_eq!(two.items.len(), 1);
    assert_ne!(one.items, two.items);
    assert!(two.next.is_none());
    assert_eq!(
        service
            .read(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                &first.id
            )
            .await
            .unwrap()
            .1,
        b"approved report\n"
    );
    assert!(service
        .read(&recipient.caller, None, None, &id, &first.id)
        .await
        .is_err());
    insert_hierarchy(
        &store,
        "sibling",
        "sibling-tree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let sibling_id = AgentId("sibling".into());
    let sibling = service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &sibling_id,
            "sibling",
            body("sibling.txt"),
        )
        .await
        .unwrap();
    assert!(service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &sibling_id,
            &page()
        )
        .await
        .is_err());
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &sibling_id,
            &sibling.id
        )
        .await
        .is_err());
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &sibling.id
        )
        .await
        .is_err());
    let stored = store
        .agent_artifact(&ArtifactBinding::from_snapshot(&before).unwrap(), &first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.authorization.actor, owner.actor);
    assert_eq!(
        stored.authorization.owner,
        ResourceOwner::Person {
            subject: owner.caller.subject.clone()
        }
    );
    *authority.reply.lock().unwrap() = Reply::Missing;
    assert!(service
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &page()
        )
        .await
        .is_err());
    *authority.reply.lock().unwrap() = Reply::Outage;
    assert!(matches!(
        service
            .read(
                &recipient.caller,
                Some(&recipient),
                Some("request-credential"),
                &id,
                &first.id
            )
            .await,
        Err(ResourceAccessError::Membership(
            MembershipError::Unavailable
        ))
    ));
    access
        .change_share(&owner, None, revoke(&selector, &share))
        .await
        .unwrap();
    assert!(service
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("request-credential"),
            &id,
            &second.id
        )
        .await
        .is_err());
    assert_eq!(
        service
            .read(&owner.caller, None, None, &id, &first.id)
            .await
            .unwrap()
            .0,
        first
    );
    assert_eq!(
        store
            .resource_access_snapshot(&selector.target())
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
    assert_eq!(store.agent_log(&id, 0, 200).await.unwrap(), logs);
}

struct BufferedArtifacts {
    store: Arc<InMemorySandboxStore>,
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    gate_publish: bool,
    gate_read: bool,
}
#[async_trait::async_trait]
impl AgentArtifactStore for BufferedArtifacts {
    async fn publish_artifact(
        &self,
        request: &ArtifactPublication,
    ) -> Result<StoredAgentArtifact, ResourceAccessError> {
        if self.gate_publish {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        self.store.publish_artifact(request).await
    }
    async fn agent_artifacts(
        &self,
        binding: &ArtifactBinding,
        page: &ArtifactPageRequest,
    ) -> Result<Vec<StoredAgentArtifactMetadata>, ResourceAccessStoreError> {
        let result = self.store.agent_artifacts(binding, page).await?;
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(result)
    }
    async fn agent_artifact(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifact>, ResourceAccessStoreError> {
        let result = self.store.agent_artifact(binding, id).await?;
        if self.gate_read {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        Ok(result)
    }
}

#[path = "artifacts_tests/authorization.rs"]
mod authorization;
#[path = "artifacts_tests/capacity.rs"]
mod capacity;
