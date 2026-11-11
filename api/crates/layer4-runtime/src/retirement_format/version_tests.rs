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
    descriptor.version = 3;
    fixture.publish(&descriptor);
    assert!(select(&fixture.config).is_err());
    assert!(installation_marker(&fixture.config).is_err());
    assert!(!fixture.config.state_root.join(DATABASE).exists());
    assert!(!fixture.config.state_root.join("home-binding").exists());
}
