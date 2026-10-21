use super::*;
use futures_util::TryStreamExt as _;
use layer2_core::sandbox::store::AgentProvenanceInventoryStore;
use layer2_core::sandbox::AgentListQuery;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: format!("fixture-{subject}"),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    }
}
struct Authority(AtomicBool);
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        if caller.subject.as_str() != "owner" && !self.0.load(Ordering::SeqCst) {
            return Err(MembershipError::NotMember);
        }
        let mut current = context(caller.subject.as_str());
        let now = SystemTime::now();
        current.memberships.push(OrganizationMembership::new(
            current.credential.clone(),
            organization.clone(),
            OrganizationRole::Member,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(current)
    }
}
async fn inventory(
    store: &MongoSandboxStore,
    subject: &SubjectId,
    organizations: &[SubjectId],
) -> Vec<AgentId> {
    store
        .scan_shared_agent_provenance_inventory(
            subject,
            organizations,
            &AgentListQuery::default(),
            &PageRequest::new(1, None).unwrap(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.provenance.resource.lineage.agent.unwrap().id)
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn typed_links_persist_redeem_and_revoke_without_migrating_legacy_shares() {
    let (client, name) = database("typed_share_links").await;
    let (one, target) = fixture(&client, &name).await;
    let before = one.resource_access_snapshot(&target).await.unwrap();
    let id = before.lineage.agent.as_ref().unwrap().id.clone();
    let logs = one.agent_log(&id, 0, 20).await.unwrap();
    let legacy = one
        .mutate_share(mutation(&before, "legacy", ShareMode::View))
        .await
        .unwrap();
    // Model the pre-feature database: legacy documents, without the new index
    // or a link/accepted_subjects field. Startup must not rewrite those records.
    let legacy_document = one
        .resource_shares
        .find_one(doc! {"_id": &legacy.share.id})
        .await
        .unwrap()
        .unwrap();
    assert!(!legacy_document.contains_key("accepted_subjects"));
    assert!(!legacy_document
        .get_document("record")
        .unwrap()
        .contains_key("link"));
    one.resource_shares
        .drop_index("resource_share_link_recipient")
        .await
        .unwrap();
    let two = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    assert_eq!(
        two.resource_share(&legacy.share.id).await.unwrap(),
        Some(legacy.share.clone())
    );
    assert_eq!(
        two.resource_shares
            .find_one(doc! {"_id": &legacy.share.id})
            .await
            .unwrap(),
        Some(legacy_document)
    );
    let indexes: Vec<_> = two
        .resource_shares
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(indexes.iter().any(
        |index| index.options.as_ref().and_then(|o| o.name.as_deref())
            == Some("resource_share_link_recipient")
            && index.keys
                == doc! {"accepted_subjects":1,"revoked":1,"selector_kind":1,"selector_id":1}
    ));
    let authority = Arc::new(Authority(AtomicBool::new(true)));
    let directory = layer2_core::UserDirectory::new(Arc::new(
        crate::store::MongoUserDirectoryStore::open(&client, &name)
            .await
            .unwrap(),
    ));
    let recipient = context("recipient");
    directory
        .observe(&layer2_core::PublicIdentity {
            subject: recipient.caller.subject.clone(),
            display_name: Some("Recipient".into()),
            avatar_url: None,
        })
        .await
        .unwrap();
    let one = Arc::new(one);
    let first = ResourceAccessService::new(one.clone(), authority.clone())
        .with_directory(directory.clone());
    let second =
        ResourceAccessService::new(two.clone(), authority.clone()).with_directory(directory);
    let owner = context("owner");
    first
        .change_share(
            &owner,
            None,
            ShareCommand {
                selector: legacy.share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: legacy.share.id.clone(),
                },
                expected_share_revision: Some(legacy.share.revision),
                idempotency_key: "revoke-legacy".into(),
            },
        )
        .await
        .unwrap();
    let organization = SubjectId::new("org");
    for (index, audience) in [
        AgentShareAudience::Public,
        AgentShareAudience::Users {
            subjects: vec![recipient.caller.subject.clone()],
        },
        AgentShareAudience::Organization {
            organization_id: organization.clone(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let token = format!("{:064x}", index + 1);
        let command = || CreateAgentShareLink {
            agent_id: id.clone(),
            audience: audience.clone(),
            mode: ShareMode::ViewWrite,
            token: token.clone(),
            idempotency_key: format!("link-{index}"),
        };
        let created = first
            .create_agent_share_link(&owner, Some("credential"), command())
            .await
            .unwrap();
        assert_eq!(
            second
                .create_agent_share_link(&owner, Some("credential"), command())
                .await
                .unwrap(),
            created
        );
        assert!(inventory(&two, &recipient.caller.subject, &[])
            .await
            .is_empty());
        assert!(second
            .redeem_agent_share_link(
                &recipient,
                Some("credential"),
                &created.share.id,
                &"f".repeat(64)
            )
            .await
            .is_err());
        if matches!(audience, AgentShareAudience::Users { .. }) {
            assert!(second
                .redeem_agent_share_link(
                    &context("stranger"),
                    Some("credential"),
                    &created.share.id,
                    &token
                )
                .await
                .is_err());
        }
        // Two replicas redeem the same link concurrently. Exactly one principal
        // entry and one accepted revision must survive an ambiguous retry.
        let (a, b) = tokio::join!(
            first.redeem_agent_share_link(
                &recipient,
                Some("credential"),
                &created.share.id,
                &token
            ),
            second.redeem_agent_share_link(
                &recipient,
                Some("credential"),
                &created.share.id,
                &token
            ),
        );
        let accepted = a.unwrap();
        assert_eq!(b.unwrap(), accepted);
        assert_eq!(accepted.revision, 2);
        assert_eq!(
            accepted.link.as_ref().unwrap().accepted_subjects,
            std::slice::from_ref(&recipient.caller.subject)
        );
        assert_eq!(
            two.resource_share(&accepted.id).await.unwrap(),
            Some(accepted.clone())
        );
        let document = two
            .resource_shares
            .find_one(doc! {"_id": &accepted.id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            document.get_array("accepted_subjects").unwrap(),
            &vec![Bson::String("recipient".into())]
        );
        assert_eq!(
            accepted.link.as_ref().unwrap().token_sha256,
            format!("{:x}", Sha256::digest(token.as_bytes()))
        );
        assert!(!serde_json::to_string(&document).unwrap().contains(&token));
        let organizations = if matches!(audience, AgentShareAudience::Organization { .. }) {
            vec![organization.clone()]
        } else {
            vec![]
        };
        assert_eq!(
            inventory(&two, &recipient.caller.subject, &organizations).await,
            std::slice::from_ref(&id)
        );
        assert!(inventory(&two, &owner.caller.subject, &organizations)
            .await
            .is_empty());
        if !organizations.is_empty() {
            authority.0.store(false, Ordering::SeqCst);
            assert!(inventory(&two, &recipient.caller.subject, &[])
                .await
                .is_empty());
            assert!(second
                .authorize(
                    &recipient,
                    Some("credential"),
                    &target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read
                )
                .await
                .is_err());
            authority.0.store(true, Ordering::SeqCst);
        }
        second
            .authorize(
                &recipient,
                Some("credential"),
                &target,
                ResourceCapability::OperateAgent,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await
            .unwrap();
        let revoked = first
            .change_share(
                &owner,
                None,
                ShareCommand {
                    selector: accepted.selector.clone(),
                    change: ShareChange::Revoke {
                        share_id: accepted.id.clone(),
                    },
                    expected_share_revision: Some(accepted.revision),
                    idempotency_key: format!("revoke-{index}"),
                },
            )
            .await
            .unwrap();
        assert!(revoked.share.revoked_at.is_some());
        assert!(inventory(&two, &recipient.caller.subject, &organizations)
            .await
            .is_empty());
        assert!(second
            .redeem_agent_share_link(&recipient, Some("credential"), &accepted.id, &token)
            .await
            .is_err());
        assert!(second
            .authorize(
                &recipient,
                Some("credential"),
                &target,
                ResourceCapability::ViewHistory,
                AccessPurpose::Read
            )
            .await
            .is_err());
    }
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .resource_access_snapshot(&target)
            .await
            .unwrap()
            .lineage,
        before.lineage
    );
    assert_eq!(reopened.agent_log(&id, 0, 20).await.unwrap(), logs);
    assert!(
        inventory(&reopened, &recipient.caller.subject, &[organization])
            .await
            .is_empty()
    );
    client.database(&name).drop().await.unwrap();
}
