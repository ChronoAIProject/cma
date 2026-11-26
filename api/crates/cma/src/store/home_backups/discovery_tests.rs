use super::*;
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{Sandbox, SandboxHome};
use layer3_adapter::agent_sandbox::*;
use layer3_adapter::{HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt};
use std::collections::BTreeMap;

pub(super) fn retained(id: &str, native: bool) -> Sandbox {
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(id);
    let policy = HomeVolumePolicy {
        mode: if native {
            HomeVolumeMode::NativeVctEmptyV1
        } else {
            HomeVolumeMode::LegacySnapshot
        },
        storage_class: "storage".into(),
        capacity_gib: 64,
        template_name: if native { "" } else { "seed" }.into(),
        template_generation: if native { "" } else { "seed-uid" }.into(),
    };
    let mut home = SandboxHome::reserve(&sandbox.id, policy.clone()).unwrap();
    let origin = native.then(|| {
        Box::new(NativePoolHomeOrigin {
            claim: NativePoolClaim {
                request: NativePoolClaimRequest {
                    pool: NativePoolBinding {
                        spec: NativePoolSpec {
                            cold_canary: false,
                            image: layer3_adapter::RuntimeImage {
                                uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                                labels: BTreeMap::from([
                                    (layer3_adapter::RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                                    ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                                ]),
                            },
                            cpu_millis: 1000,
                            memory_mib: 2048,
                            home: policy,
                        },
                        pod_policy: NativePoolPodPolicy {
                            runtime_class: None,
                            service_account: "runtime".into(),
                            image_pull_secrets: vec![],
                            template_network_policy: None,
                            fs_group_change_policy: None,
                        },
                        name: "pool".into(),
                        uid: "pool-uid".into(),
                        template_name: "template".into(),
                        template_uid: "template-uid".into(),
                        config_sha256: "a".repeat(64),
                    },
                    home: home.request.clone(),
                    runtime_generation: 1,
                },
                name: "claim".into(),
                uid: "claim-uid".into(),
                resource_version: "1".into(),
                sandbox_name: Some("member".into()),
            },
            sandbox_uid: "member-uid".into(),
            pvc_uid: "source-fixture".into(),
        })
    });
    home.bind(HomeVolumeReceipt {
        native_authority: origin,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "source-fixture".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox
}

async fn insert(fixture: &Fixture, sandbox: &Sandbox) -> Document {
    let row = crate::store::sandboxes::sandbox_document(sandbox, 0).unwrap();
    fixture
        .client
        .database(&fixture.database)
        .collection::<Document>("sandboxes")
        .insert_one(row.clone())
        .await
        .unwrap();
    row
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn keyset_backfill_keeps_legacy_in_denominator_and_never_mutates_sandbox() {
    let fixture = Fixture::new().await;
    let native = retained("a-native", true);
    let original = insert(&fixture, &native).await;
    insert(&fixture, &retained("b-legacy", false)).await;
    let mut released = retained("c-released", false);
    released.status = layer2_core::sandbox::SandboxStatus::Retiring;
    released.work.step = layer2_core::sandbox::SandboxWorkStep::StopAgents;
    released.lifecycle.home.as_mut().unwrap().released = true;
    insert(&fixture, &released).await;
    let mut unbound = retained("d-unbound", false);
    unbound.lifecycle.home.as_mut().unwrap().receipt = None;
    insert(&fixture, &unbound).await;
    let now = layer2_core::now_rfc3339();
    let first = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, None).unwrap(), &now)
        .await
        .unwrap();
    assert_eq!(first.next.as_deref(), Some("a-native"));
    assert_eq!(first.items.len(), 1);
    assert!(first.items[0].schedule_present);
    assert!(first.items[0].latest_ready_at.is_none());
    let schedule = fixture
        .store
        .get_schedule("a-native")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(schedule.next_due_at, now);
    let mut ready = standalone_backup(HomeBackupState::Ready);
    ready.sandbox_id = "a-native".into();
    ready.id = "hb-a-native-1".into();
    ready.source.home_source_id = schedule.home.home_source_id.clone();
    ready.source.home_identity = schedule.home.home_identity.clone();
    ready.cut.requested_at = now.clone();
    ready.cut.source_creation_time = Some(now.clone());
    ready.cut.ready_at = Some(now.clone());
    ready.retention.expires_at = until();
    fixture
        .store
        .backups
        .insert_one(backup_document(&ready).unwrap())
        .await
        .unwrap();
    let again = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, None).unwrap(), &now)
        .await
        .unwrap();
    assert_eq!(
        again.items[0].latest_ready_at.as_deref(),
        Some(now.as_str())
    );
    assert_eq!(
        fixture.store.get_schedule("a-native").await.unwrap(),
        Some(schedule)
    );
    ready.retention.expires_at = now.clone();
    fixture
        .store
        .backups
        .replace_one(doc! {"_id": &ready.id}, backup_document(&ready).unwrap())
        .await
        .unwrap();
    let expired = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, None).unwrap(), &now)
        .await
        .unwrap();
    assert!(expired.items[0].latest_ready_at.is_none());
    ready.retention.expires_at = until();
    ready.state = HomeBackupState::Expiring;
    fixture
        .store
        .backups
        .replace_one(doc! {"_id": &ready.id}, backup_document(&ready).unwrap())
        .await
        .unwrap();
    let expiring = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, None).unwrap(), &now)
        .await
        .unwrap();
    assert!(expiring.items[0].latest_ready_at.is_none());
    let second = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, first.next).unwrap(), &now)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].sandbox_id, "b-legacy");
    assert!(!second.items[0].schedule_present);
    assert!(fixture
        .store
        .get_schedule("b-legacy")
        .await
        .unwrap()
        .is_none());
    let last = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, second.next).unwrap(), &now)
        .await
        .unwrap();
    assert!(last.items.is_empty());
    assert_eq!(last.next.as_deref(), Some("c-released"));
    let pending = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, last.next).unwrap(), &now)
        .await
        .unwrap();
    assert_eq!(pending.items.len(), 1);
    assert_eq!(pending.items[0].sandbox_id, "d-unbound");
    assert!(!pending.items[0].schedule_present);
    assert!(pending.items[0].latest_ready_at.is_none());
    assert!(pending.next.is_none());
    let retained_row = fixture
        .client
        .database(&fixture.database)
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":"a-native"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained_row, original);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn backfill_refuses_changed_identity_invalid_pages_and_corrupt_parent() {
    let fixture = Fixture::new().await;
    insert(&fixture, &retained("a-native", true)).await;
    let now = layer2_core::now_rfc3339();
    let page = PageRequest::new(1, None).unwrap();
    fixture.store.backfill_schedules(&page, &now).await.unwrap();
    let before = fixture
        .store
        .get_schedule("a-native")
        .await
        .unwrap()
        .unwrap();
    let mut changed = before.clone();
    changed.home.home_source_id.push_str("-replacement");
    fixture
        .store
        .schedules
        .replace_one(
            doc! {"_id":"a-native"},
            schedule_document(&changed).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.store.backfill_schedules(&page, &now).await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(
        fixture.store.get_schedule("a-native").await.unwrap(),
        Some(changed)
    );
    assert!(fixture
        .store
        .backfill_schedules(
            &PageRequest {
                limit: 0,
                cursor: None
            },
            &now
        )
        .await
        .is_err());
    assert!(fixture
        .store
        .backfill_schedules(&page, "2026-11-17T08:00:00+08:00")
        .await
        .is_err());
    fixture
        .client
        .database(&fixture.database)
        .collection::<Document>("sandboxes")
        .insert_one(doc! {"_id":"b-corrupt","document_schema_version":999})
        .await
        .unwrap();
    assert!(fixture
        .store
        .backfill_schedules(&PageRequest::new(1, Some("a-native".into())).unwrap(), &now)
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a real isolated Mongo replica set"]
async fn backfill_of_retiring_parent_never_admits_a_capture() {
    let fixture = Fixture::new().await;
    let now = layer2_core::now_rfc3339();
    let mut parent = retained("retiring-native", true);
    parent.begin_retirement(now.clone()).unwrap();
    insert(&fixture, &parent).await;
    let first = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, None).unwrap(), &now)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.items[0].schedule_present);
    let schedule = fixture
        .store
        .get_schedule("retiring-native")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(schedule.state, HomeBackupScheduleState::Retiring);
    assert!(fixture
        .store
        .claim_due(&now, &holder(), &until())
        .await
        .unwrap()
        .is_none());
    let again = fixture
        .store
        .backfill_schedules(&PageRequest::new(1, None).unwrap(), &now)
        .await
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(
        fixture.store.get_schedule("retiring-native").await.unwrap(),
        Some(schedule)
    );
    fixture.cleanup().await;
}
