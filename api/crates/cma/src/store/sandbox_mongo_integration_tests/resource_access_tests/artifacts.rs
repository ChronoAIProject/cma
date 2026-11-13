use super::*;
use layer2_core::sandbox::agent_artifacts::*;
use layer2_core::{Authorization, Caller, Capability};
use std::time::SystemTime;

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: "fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    }
}
struct Authority;
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        let mut current = context(caller.subject.as_str());
        let now = SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            OrganizationRole::Viewer,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(current)
    }
}
fn body() -> PublishAgentArtifact {
    PublishAgentArtifact {
        name: "report.txt".into(),
        media_type: "text/plain".into(),
        content_base64: "YXBwcm92ZWQK".into(),
    }
}
fn page() -> ArtifactPageRequest {
    ArtifactPageRequest {
        limit: 1,
        cursor: None,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn artifact_publication_replays_across_replicas_and_revocation_preserves_retained_bytes() {
    let (client, name) = database("agent_artifact_replay").await;
    let (one, target) = fixture(&client, &name).await;
    let before = one.resource_access_snapshot(&target).await.unwrap();
    let agent = before.lineage.agent.as_ref().unwrap();
    let owner = context(before.lineage.sandbox.owner.as_str());
    let log = one.agent_log(&agent.id, 0, 20).await.unwrap();
    let two = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let one = Arc::new(one);
    let access = Arc::new(ResourceAccessService::new(one.clone(), Arc::new(Authority)));
    let first = AgentArtifactService::new(one.clone(), access.clone());
    let second = AgentArtifactService::new(
        two.clone(),
        Arc::new(ResourceAccessService::new(two.clone(), Arc::new(Authority))),
    );
    let (a, b) = tokio::join!(
        first.publish(&owner.caller, Some(&owner), None, &agent.id, "same", body()),
        second.publish(&owner.caller, Some(&owner), None, &agent.id, "same", body()),
    );
    let metadata = a.unwrap();
    assert_eq!(b.unwrap(), metadata);
    assert_eq!(
        one.agent_artifacts.count_documents(doc! {}).await.unwrap(),
        1
    );
    let other = first
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &agent.id,
            "other",
            body(),
        )
        .await
        .unwrap();
    let share = one
        .mutate_share(mutation(&before, "view", ShareMode::View))
        .await
        .unwrap()
        .share;
    let recipient = context("recipient");
    let first_page = second
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("credential"),
            &agent.id,
            &page(),
        )
        .await
        .unwrap();
    assert_eq!(first_page.items.len(), 1);
    let last = second
        .list(
            &recipient.caller,
            Some(&recipient),
            Some("credential"),
            &agent.id,
            &ArtifactPageRequest {
                cursor: first_page.next.clone(),
                ..page()
            },
        )
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert!(last.next.is_none());
    assert_ne!(last.items, first_page.items);
    assert_eq!(
        second
            .metadata(
                &recipient.caller,
                Some(&recipient),
                Some("credential"),
                &agent.id,
                &metadata.id,
            )
            .await
            .unwrap(),
        metadata
    );
    assert_eq!(
        second
            .read(
                &recipient.caller,
                Some(&recipient),
                Some("credential"),
                &agent.id,
                &metadata.id
            )
            .await
            .unwrap()
            .1,
        b"approved\n"
    );
    access
        .change_share(
            &owner,
            None,
            ShareCommand {
                selector: share.selector.clone(),
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let reads = AgentArtifactService::new(
        reopened.clone(),
        Arc::new(ResourceAccessService::new(
            reopened.clone(),
            Arc::new(Authority),
        )),
    );
    assert!(reads
        .metadata(
            &recipient.caller,
            Some(&recipient),
            Some("credential"),
            &agent.id,
            &metadata.id,
        )
        .await
        .is_err());
    assert_eq!(
        reads
            .metadata(&owner.caller, None, None, &agent.id, &metadata.id)
            .await
            .unwrap(),
        metadata
    );
    assert!(reads
        .read(
            &recipient.caller,
            Some(&recipient),
            Some("credential"),
            &agent.id,
            &other.id
        )
        .await
        .is_err());
    assert_eq!(
        reads
            .read(&owner.caller, None, None, &agent.id, &metadata.id)
            .await
            .unwrap()
            .0,
        metadata
    );
    assert_eq!(
        reopened
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
    assert_eq!(reopened.agent_log(&agent.id, 0, 20).await.unwrap(), log);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn artifact_pages_project_metadata_but_downloads_validate_retained_bytes() {
    let (client, name) = database("agent_artifact_corrupt").await;
    let (store, target) = fixture(&client, &name).await;
    let store = Arc::new(store);
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let owner = context(before.lineage.sandbox.owner.as_str());
    let agent = before.lineage.agent.as_ref().unwrap();
    let service = AgentArtifactService::new(
        store.clone(),
        Arc::new(ResourceAccessService::new(
            store.clone(),
            Arc::new(Authority),
        )),
    );
    let one = service
        .publish(&owner.caller, None, None, &agent.id, "one", body())
        .await
        .unwrap();
    let two = service
        .publish(&owner.caller, None, None, &agent.id, "two", body())
        .await
        .unwrap();
    let hidden = if one.id > two.id { one.id } else { two.id };
    store
        .agent_artifacts
        .update_one(
            doc! {"_id":&hidden},
            doc! {"$set":{"record.content_base64":"dGFtcGVyZWQ="}},
        )
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let service = AgentArtifactService::new(
        reopened.clone(),
        Arc::new(ResourceAccessService::new(
            reopened.clone(),
            Arc::new(Authority),
        )),
    );
    // Metadata listing never fetches/decodes content, even in its lookahead row.
    // The separately authorized download still checks the retained byte digest.
    let listed = service
        .list(&owner.caller, None, None, &agent.id, &page())
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert!(listed.next.is_some());
    assert!(!serde_json::to_string(&listed.items)
        .unwrap()
        .contains("content_base64"));
    assert!(matches!(
        service
            .read(&owner.caller, None, None, &agent.id, &hidden)
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt
        ))
    ));
    // A metadata-only page still validates every selected record and lookahead.
    reopened
        .agent_artifacts
        .update_one(
            doc! {"_id":&hidden},
            doc! {"$set":{"record.metadata.sha256":"invalid"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .list(&owner.caller, None, None, &agent.id, &page())
            .await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt
        ))
    ));
    assert_eq!(
        reopened
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
    client.database(&name).drop().await.unwrap();
}
