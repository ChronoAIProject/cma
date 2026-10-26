use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    Authorization, Caller, Capability, Page, PublicIdentity, RegisteredUser, StoreError, SubjectId,
    UserDirectory, UserDirectoryStore,
};

use super::tests::{content, seed_profiles, trigger_store, Visibilities};
use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

struct DirectoryStub {
    response: Result<Option<RegisteredUser>, StoreError>,
}

#[async_trait]
impl UserDirectoryStore for DirectoryStub {
    async fn observe(&self, _identity: &PublicIdentity) -> Result<(), StoreError> {
        Ok(())
    }

    async fn user(&self, _subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        self.response.clone()
    }

    async fn list(
        &self,
        _query: &crate::DirectoryQuery,
    ) -> Result<Page<RegisteredUser>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next_cursor: None,
        })
    }
}

fn directory(response: Result<Option<RegisteredUser>, StoreError>) -> Arc<UserDirectory> {
    UserDirectory::new(Arc::new(DirectoryStub { response }))
}

fn publisher(subject: SubjectId) -> Caller {
    Caller::new(subject, Authorization::new([Capability::PublishTriggers]))
}

fn visitor_launch(trigger: &PublicTrigger, placement_id: Option<PlacementId>, key: char) -> Launch {
    Launch::new(LaunchSeed {
        trigger_id: trigger.trigger_id.clone(),
        revision_digest: trigger.revision.digest.clone(),
        public_id: trigger.public_id.clone(),
        owner: SubjectId::new("visitor"),
        authority: LaunchAuthorityClass::SandboxCreator,
        placement_id,
        referrer_origin: None,
        requirements: RequirementsSnapshot {
            services: Vec::new(),
            checked_at: NOW.into(),
        },
        review: ReviewRequirements {
            needs: Vec::new(),
            can_start_now: true,
        },
        trust: LaunchTrustManifest::none(),
        resolution: LaunchResolution::pending(),
        has_first_message: true,
        command_key_hash: key.to_string().repeat(64),
        command_fingerprint: key.to_ascii_uppercase().to_string().repeat(64),
        created_at: NOW.into(),
    })
    .unwrap()
}

#[tokio::test]
async fn collections_require_publish_authority_while_owner_operations_use_ownership() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let definitions = Arc::new(trigger_store(&seed));
    let launches = Arc::new(InMemoryLaunchStore::new(definitions.clone()));
    let service = TriggerService::new(definitions, launches, directory(Ok(None)));
    let publisher = publisher(seed.owner.clone());
    let denied = Caller::new(seed.owner.clone(), Authorization::none());
    assert_eq!(
        service.create(&denied, content(seed.target.clone())).await,
        Err(TriggerServiceError::Forbidden)
    );
    assert_eq!(
        service.list(&denied, &TriggerPageRequest::default()).await,
        Err(TriggerServiceError::Forbidden)
    );

    let created = service
        .create(&publisher, content(seed.target.clone()))
        .await
        .unwrap();
    assert_eq!(created.owner, seed.owner);
    assert_eq!(
        service
            .list(&publisher, &TriggerPageRequest::default())
            .await
            .unwrap()
            .items,
        vec![created.clone()]
    );
    let owner = Caller::new(seed.owner.clone(), Authorization::none());
    assert_eq!(service.read(&owner, &created.id).await.unwrap(), created);
    let foreign = Caller::new(SubjectId::new("different-publisher"), Authorization::none());
    assert_eq!(
        service.read(&foreign, &created.id).await,
        Err(TriggerServiceError::NotFound)
    );

    let mut changed = created.draft.clone();
    changed.presentation.title = "Changed title".into();
    let updated = service
        .update_draft(&owner, &created.id, 1, changed)
        .await
        .unwrap();
    assert_eq!(updated.draft_version, 2);
    let placement = service
        .create_placement(
            &owner,
            &created.id,
            "https://example.com/readme#run".into(),
            "README".into(),
        )
        .await
        .unwrap();
    assert_eq!(placement.source_url, "https://example.com/readme");
    service
        .retire_placement(&owner, &created.id, &placement.id)
        .await
        .unwrap();
    assert!(service
        .placements(&owner, &created.id, false)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        service
            .placements(&owner, &created.id, true)
            .await
            .unwrap()
            .len(),
        1
    );

    let published = service.publish(&owner, &created.id, 2).await.unwrap();
    assert_eq!(
        service.revisions(&owner, &created.id).await.unwrap(),
        vec![published.revision]
    );
    assert_eq!(
        service
            .retire(&owner, &created.id)
            .await
            .unwrap()
            .availability,
        TriggerAvailability::Retired
    );

    service.delete(&owner, &created.id).await.unwrap();
    assert_eq!(
        service.read(&owner, &created.id).await,
        Err(TriggerServiceError::NotFound)
    );

    let disposable = service
        .create(&publisher, content(seed.target))
        .await
        .unwrap();
    service.delete(&owner, &disposable.id).await.unwrap();
    assert_eq!(
        service.read(&owner, &disposable.id).await,
        Err(TriggerServiceError::NotFound)
    );
}

#[tokio::test]
async fn statistics_include_zero_and_unattributed_rows_and_match_transactional_totals() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let definitions = Arc::new(trigger_store(&seed));
    let launches = Arc::new(InMemoryLaunchStore::new(definitions.clone()));
    let service = TriggerService::new(definitions, launches.clone(), directory(Ok(None)));
    let owner = publisher(seed.owner.clone());
    let trigger = service
        .create(&owner, content(seed.target.clone()))
        .await
        .unwrap();
    let used = service
        .create_placement(
            &owner,
            &trigger.id,
            "https://example.com/used".into(),
            "Used".into(),
        )
        .await
        .unwrap();
    let unused = service
        .create_placement(
            &owner,
            &trigger.id,
            "https://example.com/unused".into(),
            "Unused".into(),
        )
        .await
        .unwrap();
    let public = service.publish(&owner, &trigger.id, 1).await.unwrap();
    for (placement_id, key) in [(Some(used.id.clone()), 'a'), (None, 'b')] {
        launches
            .admit_launch(LaunchAdmission {
                launch: visitor_launch(&public, placement_id, key),
                trigger: public.clone(),
                events: vec![LaunchEventKind::Created],
            })
            .await
            .unwrap();
    }

    let statistics = service.statistics(&owner, &trigger.id).await.unwrap();
    assert_eq!(
        statistics.totals,
        TriggerStats {
            launches_total: 2,
            agents_ready_total: 0,
        }
    );
    assert_eq!(
        statistics.placements,
        vec![
            LaunchPlacementStats {
                placement_id: Some(used.id),
                launches_total: 1,
                agents_ready_total: 0,
            },
            LaunchPlacementStats {
                placement_id: Some(unused.id),
                launches_total: 0,
                agents_ready_total: 0,
            },
            LaunchPlacementStats {
                placement_id: None,
                launches_total: 1,
                agents_ready_total: 0,
            },
        ]
    );
    assert_eq!(
        service
            .statistics(&publisher(SubjectId::new("stranger")), &trigger.id)
            .await,
        Err(TriggerServiceError::NotFound)
    );
}

#[tokio::test]
async fn statistics_refuse_unknown_placements_and_counter_drift() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let definitions = Arc::new(trigger_store(&seed));
    let launches = Arc::new(InMemoryLaunchStore::new(definitions.clone()));
    let service = TriggerService::new(definitions.clone(), launches.clone(), directory(Ok(None)));
    let owner = publisher(seed.owner.clone());
    let trigger = service
        .create(&owner, content(seed.target.clone()))
        .await
        .unwrap();
    let public = service.publish(&owner, &trigger.id, 1).await.unwrap();
    launches
        .admit_launch(LaunchAdmission {
            launch: visitor_launch(
                &public,
                Some(PlacementId("plc_not_owned_by_trigger".into())),
                'a',
            ),
            trigger: public,
            events: vec![LaunchEventKind::Created],
        })
        .await
        .unwrap();
    let error = service.statistics(&owner, &trigger.id).await.unwrap_err();
    assert_eq!(error.code(), "stored_trigger_corrupt");
    assert!(error.to_string().contains("unknown placement"));

    let clean = service.create(&owner, content(seed.target)).await.unwrap();
    service.publish(&owner, &clean.id, 1).await.unwrap();
    definitions
        .record_launch(&clean.id)
        .await
        .expect("the store counter can expose drift independently");
    let error = service.statistics(&owner, &clean.id).await.unwrap_err();
    assert_eq!(error.code(), "stored_trigger_corrupt");
    assert!(error.to_string().contains("do not match"));
}

#[tokio::test]
async fn preview_uses_directory_name_without_exposing_the_publisher_subject() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let definitions = Arc::new(trigger_store(&seed));
    let launches = Arc::new(InMemoryLaunchStore::new(definitions.clone()));
    let owner = publisher(seed.owner.clone());
    let base = TriggerService::new(definitions.clone(), launches.clone(), directory(Ok(None)));
    let trigger = base.create(&owner, content(seed.target)).await.unwrap();
    let public = base.publish(&owner, &trigger.id, 1).await.unwrap();

    let fallback = base.preview(&public.public_id).await.unwrap();
    assert_eq!(fallback.publisher_display_name, "CMA publisher");
    assert_ne!(fallback.publisher_display_name, seed.owner.as_str());

    let named = TriggerService::new(
        definitions.clone(),
        launches.clone(),
        directory(Ok(Some(RegisteredUser {
            identity: PublicIdentity {
                subject: seed.owner.clone(),
                display_name: Some("Ada Lovelace".into()),
                avatar_url: None,
            },
            first_seen_at: NOW.into(),
            last_seen_at: NOW.into(),
        }))),
    );
    assert_eq!(
        named
            .preview(&public.public_id)
            .await
            .unwrap()
            .publisher_display_name,
        "Ada Lovelace"
    );

    let unavailable = TriggerService::new(
        definitions,
        launches,
        directory(Err(StoreError::Unavailable("directory offline".into()))),
    );
    assert_eq!(
        unavailable
            .preview(&public.public_id)
            .await
            .unwrap_err()
            .code(),
        "trigger_service_unavailable"
    );
}
