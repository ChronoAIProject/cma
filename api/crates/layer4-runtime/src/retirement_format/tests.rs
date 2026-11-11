use super::*;
#[path = "fixture.rs"]
mod fixture;
use crate::mutation_store::{Begin, Store};
use fixture::*;
use std::sync::Arc;

#[tokio::test]
async fn unactivated_home_stays_absent_and_a_late_publication_holds() {
    let fixture = Fixture::new();
    assert!(select(&fixture.config)
        .unwrap()
        .open(&fixture.config)
        .await
        .unwrap()
        .is_none());
    let store = Store::new(fixture.config.clone(), None);
    store.verify().unwrap();
    write(&fixture.config.state_root.join(TEMP), b"incomplete");
    assert!(store.verify().is_err());
    assert!(store.admit(1, &[operation("live")]).await.is_err());
    assert!(!fixture
        .config
        .state_root
        .join("mutation-authority")
        .exists());
    assert!(!fixture.config.state_root.join("runtime-mutations").exists());
    assert!(!fixture.config.state_root.join("home-binding").exists());
}

#[tokio::test]
async fn partial_or_missing_dependency_holds_before_home_initialization() {
    for artifact in [MANIFEST, DATABASE, STAGE, TEMP, BARRIER] {
        let fixture = Fixture::new();
        let path = if artifact == BARRIER {
            fixture.descriptor().marker()
        } else {
            fixture.config.state_root.join(artifact)
        };
        write(&path, b"incomplete");
        assert!(crate::supervisor::Supervisor::start(
            fixture.config.clone(),
            crate::writer_fence::SupervisorBoundary::default(),
        )
        .await
        .is_err());
        assert!(!fixture.config.state_root.join("home-binding").exists());
    }
    let fixture = Fixture::new();
    fixture.publish(&fixture.descriptor());
    assert!(crate::supervisor::Supervisor::start(
        fixture.config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .await
    .is_err());
    assert!(!fixture.config.state_root.join("home-binding").exists());
    assert!(!fixture.config.state_root.join(DATABASE).exists());
}

#[tokio::test]
async fn unfinished_deactivation_blocks_startup_and_releases_the_archive_lock() {
    let fixture = Fixture::new();
    let proof = fixture.activate(false).await;
    assert!(select(&fixture.config)
        .unwrap()
        .open(&fixture.config)
        .await
        .is_err());
    let archive = Archive::open(
        fixture.config.state_root.join(DATABASE),
        fixture.config.clone(),
        fixture.id.clone(),
        false,
        PAGES,
    )
    .await
    .unwrap();
    archive
        .complete(proof.key, proof.source_sha256)
        .await
        .unwrap();
    archive.close().await.unwrap();
    fixture.reader().await.archive.close().await.unwrap();
}

#[tokio::test]
async fn both_active_formats_refuse_either_retired_intent_before_any_admission() {
    for indexed in [false, true] {
        let fixture = Fixture::new();
        let ledger = fixture.ledger(indexed).await;
        fixture.activate(true).await;
        let reader = fixture.reader().await;
        let store =
            Store::new(fixture.config.clone(), ledger.clone()).with_retirement(Some(reader));
        let live = operation("live");
        let retired = operation("retired");
        for pair in [
            [live.clone(), retired.clone()],
            [retired.clone(), live.clone()],
        ] {
            assert_eq!(
                store.admit(10, &pair).await.unwrap_err().code,
                "mutation_resource_retired"
            );
            assert!(!store.admitted(&live).await.unwrap());
            if let Some(ledger) = &ledger {
                assert!(ledger.recovery_fence(live.clone()).await.unwrap().is_none());
            } else {
                assert!(!fixture
                    .config
                    .state_root
                    .join("mutation-authority")
                    .exists());
            }
        }
        assert_eq!(
            store
                .begin::<serde_json::Value>(&retired)
                .await
                .unwrap_err()
                .code,
            "mutation_resource_retired"
        );
        store.admit(10, std::slice::from_ref(&live)).await.unwrap();
        assert!(matches!(
            store.begin::<serde_json::Value>(&live).await.unwrap(),
            Begin::New(_)
        ));
        // Drop both storage owners explicitly before removing the fixture.
        store.close_retirement().await.unwrap();
        if let Some(ledger) = ledger {
            Arc::try_unwrap(ledger).ok().unwrap().close().await.unwrap();
        }
    }
}

#[tokio::test]
async fn removed_or_replaced_metadata_or_archive_holds_an_open_reader() {
    for target in [MANIFEST, BARRIER, DATABASE] {
        let fixture = Fixture::new();
        fixture.activate(true).await;
        let reader = fixture.reader().await;
        let path = if target == BARRIER {
            fixture.descriptor().marker()
        } else {
            fixture.config.state_root.join(target)
        };
        let saved = fixture.root.join("saved");
        std::fs::rename(&path, &saved).unwrap();
        assert!(reader.authorize(&[operation("live")]).await.is_err());
        if target != DATABASE {
            write(&path, &std::fs::read(&saved).unwrap());
            // Even equal bytes in a new inode do not change this process's view.
            assert!(reader.authorize(&[operation("live")]).await.is_err());
        }
        reader.archive.close().await.unwrap();
    }
}

#[tokio::test]
async fn active_store_and_home_binding_cannot_change_under_a_retirement_descriptor() {
    let fixture = Fixture::new();
    let ledger = fixture.ledger(true).await.unwrap();
    fixture.activate(true).await;
    let reader = fixture.reader().await;
    let active = fixture
        .config
        .state_root
        .join("mutation-ledger-v2.current.json");
    let original = std::fs::read(&active).unwrap();
    std::fs::write(&active, b"{}").unwrap();
    assert!(reader.verify().is_err());
    assert!(select(&fixture.config).is_err());
    std::fs::write(&active, original).unwrap();
    reader.verify().unwrap();
    for change in 0..5 {
        let mut config = fixture.config.clone();
        match change {
            0 => config.sandbox_id = "foreign".into(),
            1 => config.home_identity = Some("b".repeat(43)),
            2 => config.home = fixture.root.join("foreign-home"),
            3 => config.generation = 0,
            _ => config.checkpoint_root = fixture.root.join("foreign-transfer"),
        }
        assert!(select(&config).is_err());
    }
    reader.archive.close().await.unwrap();
    Arc::try_unwrap(ledger).ok().unwrap().close().await.unwrap();
}

#[tokio::test]
async fn frozen_installation_reader_refuses_before_changing_started_installation() {
    let fixture = Fixture::new();
    let request = layer3_adapter::InstallRequest::new("retirement-reader-test", vec![]);
    let record = crate::install_state::InstallRecord::started(&fixture.config, &request);
    crate::install_state::write_initial(&fixture.config, &record).unwrap();
    let path = fixture
        .config
        .checkpoint_root
        .join("runtime-state/install-v2.json");
    let original = std::fs::read(&path).unwrap();
    fixture.activate(true).await;
    assert_eq!(
        crate::ledger_format::v1_reader::recover(&fixture.config)
            .unwrap_err()
            .code,
        "install_state_invalid"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        crate::install_state::inspect(&fixture.config).unwrap(),
        Some(record)
    );
}
