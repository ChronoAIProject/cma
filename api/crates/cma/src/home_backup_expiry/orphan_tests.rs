use super::orphans::*;
use super::*;
use layer2_core::sandbox::home_backup::*;
use std::{collections::BTreeMap, sync::Mutex};

#[path = "orphan_fixture.rs"]
mod fixture;
use fixture::*;

#[tokio::test]
async fn pitr_fixture_imports_lost_records_across_pages_and_singapore_midnight() {
    let before = candidate(1, "2026-11-16T15:30:00Z");
    let lost = candidate(2, "2026-11-16T16:00:00Z");
    let same_hour = candidate(3, "2026-11-16T16:15:00Z");
    let next = candidate(4, "2026-11-16T16:30:00Z");
    let fixture = Fixture::new(vec![
        vec![before.clone(), lost.clone()],
        vec![same_hour, next],
    ]);
    // Only the first row survives the simulated catalog PITR rollback.
    fixture
        .rows
        .lock()
        .unwrap()
        .insert(before.id.clone(), before.clone());
    let mut scanner = scanner(&fixture);
    assert_eq!(scanner.reconcile_page(NOW).await.unwrap(), 2);
    assert_eq!(scanner.reconcile_page(NOW).await.unwrap(), 2);
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec![(None, PAGE_SIZE), (Some("page-1".into()), PAGE_SIZE)]
    );
    {
        let rows = fixture.rows.lock().unwrap();
        assert_eq!(rows.len(), 4);
        let recovered = &rows[&lost.id];
        assert_eq!(recovered.cut, lost.cut);
        assert_eq!(recovered.source, lost.source);
        assert_eq!(recovered.custody, lost.custody);
        assert_eq!(recovered.retention.class, HomeBackupRetentionClass::Daily);
        assert_eq!(recovered.retention.expires_at, "2026-12-16T16:00:00Z");
        assert_eq!(
            rows["hb-orphan-fixture-3"].retention.class,
            HomeBackupRetentionClass::Short
        );
        assert_eq!(
            rows["hb-orphan-fixture-3"].retention.expires_at,
            "2026-11-16T18:30:00Z"
        );
        assert_eq!(rows[&before.id].retention, before.retention);
    }
    // A finished inventory starts again and imports nothing twice.
    scanner.reconcile_page(NOW).await.unwrap();
    scanner.reconcile_page(NOW).await.unwrap();
    assert_eq!(fixture.rows.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn pitr_fixture_tombstoned_orphan_is_admitted_before_effects_and_receipted() {
    let lost = candidate(1, "2026-11-16T15:30:00Z");
    let fixture = Fixture::new(vec![vec![lost.clone()]]);
    *fixture.tombstoned.lock().unwrap() = Ok(true);
    scanner(&fixture).reconcile_page(NOW).await.unwrap();
    assert_eq!(
        fixture.rows.lock().unwrap()[&lost.id].state,
        HomeBackupState::Expiring
    );
    assert_eq!(*fixture.events.lock().unwrap(), vec!["admit"]);
    fixture.settle(&lost.id, false).unwrap_err();
    assert!(fixture.rows.lock().unwrap()[&lost.id].erasure.is_none());
    fixture.settle(&lost.id, true).unwrap();
    let rows = fixture.rows.lock().unwrap();
    assert_eq!(rows[&lost.id].state, HomeBackupState::Deleted);
    assert_eq!(rows[&lost.id].cut, lost.cut);
    assert_eq!(rows[&lost.id].retention, lost.retention);
    assert_eq!(
        rows[&lost.id].erasure.as_ref().unwrap().receipt,
        "fixture-independent-copy-absence"
    );
    assert_eq!(
        *fixture.events.lock().unwrap(),
        vec!["admit", "delete", "observe", "delete", "observe", "save"]
    );
}

#[tokio::test]
async fn old_orphan_keeps_its_original_thirty_day_cap_and_pending_copy_is_not_imported() {
    let old = candidate(1, "2026-10-17T15:30:00Z");
    let fixture = Fixture::new(vec![vec![old.clone()]]);
    scanner(&fixture).reconcile_page(NOW).await.unwrap();
    let recovered = fixture.rows.lock().unwrap()[&old.id].clone();
    assert_eq!(recovered.retention.expires_at, "2026-11-16T15:30:00Z");
    assert!(recovered.retention.expires_at.as_str() < NOW);
    assert_eq!(recovered.cut, old.cut);

    let pending = Fixture::new(vec![vec![candidate(2, "2026-11-16T15:30:00Z")]]);
    pending
        .pages
        .lock()
        .unwrap()
        .get_mut(&None)
        .unwrap()
        .as_mut()
        .unwrap()
        .entries[0]
        .ready = false;
    scanner(&pending).reconcile_page(NOW).await.unwrap();
    assert!(pending.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unavailable_tombstone_authority_and_missing_metadata_leave_page_uncommitted() {
    for failure in ["tombstone", "metadata"] {
        let row = candidate(1, "2026-11-16T15:30:00Z");
        let fixture = Fixture::new(vec![vec![row]]);
        if failure == "tombstone" {
            *fixture.tombstoned.lock().unwrap() = Err(HomeBackupError::Unavailable);
        } else {
            fixture.evidence.lock().unwrap().clear();
        }
        let mut scan = scanner(&fixture);
        assert!(scan.reconcile_page(NOW).await.is_err());
        assert!(fixture.rows.lock().unwrap().is_empty());
        assert!(fixture.events.lock().unwrap().is_empty());
        assert!(scan.reconcile_page(NOW).await.is_err());
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec![(None, PAGE_SIZE), (None, PAGE_SIZE)]
        );
    }
}

#[tokio::test]
async fn reappearing_copy_for_deleted_catalog_is_not_reopened_or_clock_refreshed() {
    let row = candidate(1, "2026-11-16T15:30:00Z");
    let fixture = Fixture::new(vec![vec![row.clone()]]);
    *fixture.tombstoned.lock().unwrap() = Ok(true);
    let mut scan = scanner(&fixture);
    scan.reconcile_page(NOW).await.unwrap();
    fixture.settle(&row.id, true).unwrap();
    let deleted = fixture.rows.lock().unwrap()[&row.id].clone();
    assert_eq!(
        scan.reconcile_page(NOW).await,
        Err(HomeBackupError::Conflict)
    );
    assert_eq!(fixture.rows.lock().unwrap()[&row.id], deleted);
}

#[tokio::test]
async fn page_failure_retries_same_cursor_and_keeps_already_admitted_records_idempotent() {
    let a = candidate(1, "2026-11-16T15:30:00Z");
    let b = candidate(2, "2026-11-16T16:00:00Z");
    let fixture = Fixture::new(vec![vec![a.clone(), b.clone()]]);
    fixture.evidence.lock().unwrap().remove(&b.id);
    let mut scan = scanner(&fixture);
    scan.reconcile_page(NOW).await.unwrap_err();
    let admitted = fixture.rows.lock().unwrap()[&a.id].clone();
    fixture.evidence.lock().unwrap().insert(b.id.clone(), b);
    scan.reconcile_page(NOW).await.unwrap();
    assert_eq!(fixture.rows.lock().unwrap()[&a.id], admitted);
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec![(None, PAGE_SIZE), (None, PAGE_SIZE)]
    );
}

#[tokio::test]
async fn invalid_pages_fail_before_catalog_admission() {
    for failure in [
        "oversize",
        "duplicate",
        "empty_token",
        "invalid_time",
        "list_error",
    ] {
        let row = candidate(1, "2026-11-16T15:30:00Z");
        let fixture = Fixture::new(vec![vec![row]]);
        {
            let mut pages = fixture.pages.lock().unwrap();
            let page = pages.get_mut(&None).unwrap().as_mut().unwrap();
            match failure {
                "oversize" => page.entries = vec![page.entries[0].clone(); PAGE_SIZE as usize + 1],
                "duplicate" => page.entries.push(page.entries[0].clone()),
                "empty_token" => page.next = Some(String::new()),
                "invalid_time" => page.entries[0].creation_time = Some("invalid".into()),
                _ => {
                    pages.insert(None, Err(HomeBackupError::Unavailable));
                }
            }
        }
        assert!(
            scanner(&fixture).reconcile_page(NOW).await.is_err(),
            "{failure}"
        );
        assert!(fixture.rows.lock().unwrap().is_empty(), "{failure}");
    }
}

#[tokio::test]
async fn repeated_tokens_duplicate_objects_and_scan_bound_report_error_then_restart() {
    for failure in ["token_cycle", "object_repeat", "scan_bound"] {
        let fixture = Fixture::new(vec![vec![], vec![]]);
        let mut scan = scanner(&fixture);
        if failure == "object_repeat" {
            let observed = observation(&candidate(1, "2026-11-16T15:30:00Z"));
            let mut pages = fixture.pages.lock().unwrap();
            for page in pages.values_mut() {
                page.as_mut().unwrap().entries.push(observed.clone());
            }
            fixture.evidence.lock().unwrap().insert(
                observed.backup_id.clone(),
                candidate(1, "2026-11-16T15:30:00Z"),
            );
        }
        scan.reconcile_page(NOW).await.unwrap();
        if failure == "scan_bound" {
            // Empty pages make the page cap cheap to exercise without a clock wait.
            for index in 1..MAX_SCAN_PAGES {
                let key = format!("page-{index}");
                fixture.pages.lock().unwrap().insert(
                    Some(key),
                    Ok(OrphanPage {
                        entries: vec![],
                        next: Some(format!("page-{}", index + 1)),
                    }),
                );
                scan.reconcile_page(NOW).await.unwrap();
            }
        } else if failure == "token_cycle" {
            fixture
                .pages
                .lock()
                .unwrap()
                .get_mut(&Some("page-1".into()))
                .unwrap()
                .as_mut()
                .unwrap()
                .next = Some("page-1".into());
        }
        let reason = match failure {
            "token_cycle" => "orphan inventory token did not advance",
            "object_repeat" => "orphan inventory repeats a backup identity",
            _ => "orphan scan exceeds page bound",
        };
        assert_eq!(
            scan.reconcile_page(NOW).await,
            Err(HomeBackupError::Invalid(reason))
        );
        let admitted = fixture.rows.lock().unwrap().clone();
        // The live inventory can shrink after the failed scan. No process
        // restart is needed to observe that change and finish a new scan.
        *fixture.pages.lock().unwrap() = BTreeMap::from([(
            None,
            Ok(OrphanPage {
                entries: vec![],
                next: None,
            }),
        )]);
        assert_eq!(scan.reconcile_page(NOW).await.unwrap(), 0, "{failure}");
        assert_eq!(
            fixture.requests.lock().unwrap().last(),
            Some(&(None, PAGE_SIZE))
        );
        assert_eq!(*fixture.rows.lock().unwrap(), admitted);
    }
}

#[tokio::test]
async fn invalid_later_page_restarts_after_inventory_is_repaired() {
    for failure in [
        "empty_token",
        "oversized_token",
        "control_token",
        "oversized_page",
        "invalid_time",
        "adapter_invalid",
    ] {
        let fixture = Fixture::new(vec![vec![], vec![candidate(1, "2026-11-16T15:30:00Z")]]);
        let mut scan = scanner(&fixture);
        scan.reconcile_page(NOW).await.unwrap();
        {
            let mut pages = fixture.pages.lock().unwrap();
            let page = pages
                .get_mut(&Some("page-1".into()))
                .unwrap()
                .as_mut()
                .unwrap();
            match failure {
                "empty_token" => page.next = Some(String::new()),
                "oversized_token" => page.next = Some("x".repeat(4097)),
                "control_token" => page.next = Some("invalid\ntoken".into()),
                "oversized_page" => {
                    page.entries = vec![page.entries[0].clone(); PAGE_SIZE as usize + 1]
                }
                "invalid_time" => page.entries[0].creation_time = Some("invalid".into()),
                _ => {
                    pages.insert(
                        Some("page-1".into()),
                        Err(HomeBackupError::Invalid("invalid inventory evidence")),
                    );
                }
            }
        }
        assert!(
            matches!(
                scan.reconcile_page(NOW).await,
                Err(HomeBackupError::Invalid(_))
            ),
            "{failure}"
        );
        assert!(fixture.rows.lock().unwrap().is_empty());
        *fixture.pages.lock().unwrap() = BTreeMap::from([(
            None,
            Ok(OrphanPage {
                entries: vec![],
                next: None,
            }),
        )]);
        assert_eq!(scan.reconcile_page(NOW).await.unwrap(), 0);
        assert_eq!(
            *fixture.requests.lock().unwrap(),
            vec![
                (None, PAGE_SIZE),
                (Some("page-1".into()), PAGE_SIZE),
                (None, PAGE_SIZE)
            ]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn inventory_timeout_does_not_advance_the_scan() {
    struct Slow;
    #[async_trait::async_trait]
    impl OrphanInventory for Slow {
        async fn list_page(&self, _: Option<&str>, _: u32) -> Result<OrphanPage, HomeBackupError> {
            std::future::pending().await
        }
    }
    let fixture = Fixture::new(vec![vec![]]);
    let mut scan = OrphanScanner::new(OrphanSources {
        inventory: Arc::new(Slow),
        catalog: fixture.clone(),
    });
    assert_eq!(
        scan.reconcile_page(NOW).await,
        Err(HomeBackupError::Unavailable)
    );
    assert!(fixture.requests.lock().unwrap().is_empty());
    assert!(fixture.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn expired_listing_snapshot_restarts_without_undoing_catalog_admission() {
    let row = candidate(1, "2026-11-16T15:30:00Z");
    let fixture = Fixture::new(vec![vec![row.clone()], vec![]]);
    fixture
        .pages
        .lock()
        .unwrap()
        .insert(Some("page-1".into()), Err(HomeBackupError::Conflict));
    let mut scan = scanner(&fixture);
    scan.reconcile_page(NOW).await.unwrap();
    assert_eq!(
        scan.reconcile_page(NOW).await,
        Err(HomeBackupError::Conflict)
    );
    scan.reconcile_page(NOW).await.unwrap();
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec![
            (None, PAGE_SIZE),
            (Some("page-1".into()), PAGE_SIZE),
            (None, PAGE_SIZE)
        ]
    );
    assert_eq!(fixture.rows.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn catalog_timeout_replays_the_same_page_without_advancing_its_cursor() {
    struct Slow;
    #[async_trait::async_trait]
    impl OrphanCatalog for Slow {
        async fn reconcile_observed(
            &self,
            _: &HomeBackupOrphanObservation,
            _: &str,
        ) -> Result<(), HomeBackupError> {
            std::future::pending().await
        }
    }
    let fixture = Fixture::new(vec![vec![candidate(1, "2026-11-16T15:30:00Z")]]);
    let mut scan = OrphanScanner::new(OrphanSources {
        inventory: fixture.clone(),
        catalog: Arc::new(Slow),
    });
    assert_eq!(
        scan.reconcile_page(NOW).await,
        Err(HomeBackupError::Unavailable)
    );
    assert_eq!(
        scan.reconcile_page(NOW).await,
        Err(HomeBackupError::Unavailable)
    );
    assert_eq!(
        *fixture.requests.lock().unwrap(),
        vec![(None, PAGE_SIZE), (None, PAGE_SIZE)]
    );
}
