use super::*;

#[tokio::test]
async fn retirement_descriptor_and_database_versions_must_agree_and_v1_stays_readable() {
    for descriptor_version in [1, 2] {
        for database_version in [Version::Legacy, Version::Indexed] {
            let fixture = Fixture::new();
            Archive::open_version(
                fixture.config.state_root.join(DATABASE),
                fixture.config.clone(),
                fixture.id.clone(),
                true,
                PAGES,
                database_version,
            )
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
            let mut descriptor = fixture.descriptor();
            descriptor.version = descriptor_version;
            fixture.publish(&descriptor);
            let reader = select(&fixture.config).unwrap().open(&fixture.config).await;
            if descriptor_version == database_version as u8 {
                reader.unwrap().unwrap().close().await.unwrap();
            } else {
                assert!(reader.is_err());
                assert!(!fixture.config.state_root.join("home-binding").exists());
            }
        }
    }
}

#[tokio::test]
async fn frozen_v1_descriptor_refuses_v2_before_installation_recovery() {
    let fixture = Fixture::new();
    let request = layer3_adapter::InstallRequest::new("retirement-version-test", vec![]);
    let record = crate::install_state::InstallRecord::started(&fixture.config, &request);
    crate::install_state::write_initial(&fixture.config, &record).unwrap();
    let path = fixture
        .config
        .checkpoint_root
        .join("runtime-state/install-v2.json");
    let original = std::fs::read(&path).unwrap();
    Archive::open_version(
        fixture.config.state_root.join(DATABASE),
        fixture.config.clone(),
        fixture.id.clone(),
        true,
        PAGES,
        Version::Indexed,
    )
    .await
    .unwrap()
    .close()
    .await
    .unwrap();
    let mut descriptor = fixture.descriptor();
    descriptor.version = 2;
    fixture.publish(&descriptor);
    assert!(v1_descriptor::Descriptor::load(&fixture.config).is_err());
    installation_marker(&fixture.config).unwrap();
    fixture.reader().await.close().await.unwrap();
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_eq!(
        crate::install_state::inspect(&fixture.config).unwrap(),
        Some(record)
    );
}

#[test]
fn unknown_descriptor_version_refuses_before_archive_open_or_installation_effects() {
    let fixture = Fixture::new();
    let mut descriptor = fixture.descriptor();
    descriptor.version = 5;
    fixture.publish(&descriptor);
    assert!(select(&fixture.config).is_err());
    assert!(installation_marker(&fixture.config).is_err());
    assert!(!fixture.config.state_root.join(DATABASE).exists());
    assert!(!fixture.config.state_root.join("home-binding").exists());
}

#[tokio::test]
async fn collection_reader_requires_v2_and_rechecks_pending_intent_after_startup() {
    use crate::ledger_reclamation::CollectionEvidence;
    use layer3_adapter::runtime_ledger_collection::CollectionGroup;
    let fixture = Fixture::new();
    let ledger = fixture.ledger(true).await.unwrap();
    Archive::open_version(
        fixture.config.state_root.join(DATABASE),
        fixture.config.clone(),
        fixture.id.clone(),
        true,
        PAGES,
        Version::Collection,
    )
    .await
    .unwrap()
    .close()
    .await
    .unwrap();
    let mut descriptor = fixture.descriptor();
    descriptor.version = 3;
    assert!(descriptor.active_v2_sha256.is_some());
    fixture.publish(&descriptor);
    assert!(v1_descriptor::Descriptor::load(&fixture.config).is_err());
    // Exercise the old archive decoder directly: it cannot reinterpret format 3.
    assert!(Archive::open_version(
        fixture.config.state_root.join(DATABASE),
        fixture.config.clone(),
        fixture.id.clone(),
        false,
        PAGES,
        Version::Indexed
    )
    .await
    .is_err());
    let reader = fixture.reader().await;
    reader.require_ready().await.unwrap();
    let (binding, bytes) = crate::retirement_archive::collection_probe::artifacts(
        &fixture.config,
        &fixture.id,
        (ledger.snapshot().await.unwrap().revision as u64).max(1),
        CollectionGroup::Agent {
            agent_id: "selected".into(),
        },
    )
    .unwrap();
    let evidence = CollectionEvidence::parse(
        &binding,
        &crate::retirement_archive::group_key(&operation("selected")),
        bytes,
    )
    .unwrap();
    reader
        .archive
        .retain_collection(evidence, "a".repeat(64))
        .await
        .unwrap();
    // Descriptor identity has not changed, but every already-open serving seam
    // must stop before replay, new admission, or recovery enumeration.
    reader.verify().unwrap();
    assert!(reader.require_ready().await.is_err());
    let store =
        Store::new(fixture.config.clone(), Some(ledger.clone())).with_retirement(Some(reader));
    assert!(store.require_serving().await.is_err());
    assert!(store.admit(5, &[operation("live")]).await.is_err());
    assert!(store
        .started(&mut crate::mutation_store::Cursor::default())
        .await
        .is_err());
    store.close_retirement().await.unwrap();
    Arc::try_unwrap(ledger).ok().unwrap().close().await.unwrap();
    assert!(select(&fixture.config)
        .unwrap()
        .open(&fixture.config)
        .await
        .is_err());
}

#[test]
fn collection_descriptor_without_active_v2_refuses_before_any_store_open() {
    let fixture = Fixture::new();
    let mut descriptor = fixture.descriptor();
    descriptor.version = 3;
    assert!(descriptor.active_v2_sha256.is_none());
    fixture.publish(&descriptor);
    assert!(select(&fixture.config).is_err());
    assert!(!fixture.config.state_root.join(DATABASE).exists());
}

#[tokio::test]
async fn file_collection_reader_refuses_pending_work_and_never_adopts_v2() {
    use crate::ledger_reclamation::CollectionEvidence;
    use layer3_adapter::runtime_ledger_collection::CollectionGroup;
    let fixture = Fixture::new();
    Archive::open_version(
        fixture.config.state_root.join(DATABASE),
        fixture.config.clone(),
        fixture.id.clone(),
        true,
        PAGES,
        Version::FileCollection,
    )
    .await
    .unwrap()
    .close()
    .await
    .unwrap();
    let mut descriptor = fixture.descriptor();
    descriptor.version = 4;
    fixture.publish(&descriptor);
    assert!(v1_descriptor::Descriptor::load(&fixture.config).is_err());
    let reader = fixture.reader().await;
    reader.require_ready().await.unwrap();
    let (binding, bytes) = crate::retirement_archive::collection_probe::file_artifacts(
        &fixture.config,
        &fixture.id,
        1,
        CollectionGroup::Agent {
            agent_id: "selected".into(),
        },
    )
    .unwrap();
    let evidence = CollectionEvidence::parse(
        &binding,
        &crate::retirement_archive::group_key(&operation("selected")),
        bytes,
    )
    .unwrap();
    reader
        .archive
        .retain_collection(evidence, "a".repeat(64))
        .await
        .unwrap();
    let store = Store::new(fixture.config.clone(), None).with_retirement(Some(reader));
    assert!(store.require_serving().await.is_err());
    assert!(store.admit(5, &[operation("live")]).await.is_err());
    assert!(store
        .started(&mut crate::mutation_store::Cursor::default())
        .await
        .is_err());
    store.close_retirement().await.unwrap();
    assert!(select(&fixture.config)
        .unwrap()
        .open(&fixture.config)
        .await
        .is_err());
    let ledger = fixture.ledger(true).await.unwrap();
    let mut descriptor = fixture.descriptor();
    descriptor.version = 4;
    assert!(descriptor.active_v2_sha256.is_some());
    fixture.publish(&descriptor);
    assert!(select(&fixture.config).is_err());
    Arc::try_unwrap(ledger).ok().unwrap().close().await.unwrap();
}
