use super::*;
use layer2_core::sandbox::agent_artifacts::*;

struct NoOrganizationLookup;

#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganizationLookup {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal owner publication requires no organization lookup")
    }

    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal owner publication requires no organization lookup")
    }
}

fn owner(subject: &SubjectId) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            subject.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: subject.clone(),
            credential_id: "fixture".into(),
        },
        actor: ResourceActor::Human {
            subject: subject.clone(),
        },
        memberships: vec![],
    }
}

pub(super) async fn publish(
    f: &Fixture,
    key: &str,
) -> Result<AgentArtifactMetadata, ResourceAccessError> {
    let store = Arc::new(f.store.clone());
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(NoOrganizationLookup),
    ));
    let service = AgentArtifactService::new(store, access);
    let owner = owner(&f.before.lineage.sandbox.owner);
    service
        .publish(
            &owner.caller,
            Some(&owner),
            None,
            &f.before.lineage.agent.as_ref().unwrap().id,
            key,
            PublishAgentArtifact {
                name: "report.txt".into(),
                media_type: "text/plain".into(),
                content_base64: "YXBwcm92ZWQK".into(),
            },
        )
        .await
}

async fn artifact_known_abort_preserves_one_record_and_unknown_result_never_replays_case(
    unknown: bool,
) {
    let f = Fixture::new(true).await;
    let labels: &[&str] = if unknown {
        &[
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ]
    } else {
        &["TransientTransactionError"]
    };
    f.fail_writes(1, labels).await;
    let result = publish(&f, "artifact-cas").await;
    f.clear_failpoint().await;
    if unknown {
        assert!(matches!(
            result,
            Err(ResourceAccessError::Store(
                ResourceAccessStoreError::Unavailable
            ))
        ));
        assert_eq!(f.trace.writes(), 1);
        assert_eq!(
            f.store
                .agent_artifacts
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        f.assert_no_publication().await;
    } else {
        let metadata = result.unwrap();
        assert_eq!(publish(&f, "artifact-cas").await.unwrap(), metadata);
        assert_eq!(
            f.store
                .agent_artifacts
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            f.store.resource_access_snapshot(&f.target).await.unwrap(),
            f.before
        );
    }
    f.destroy().await;
}

#[path = "artifacts_cases.rs"]
mod cases;
