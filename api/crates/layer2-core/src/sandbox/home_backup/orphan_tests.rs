use super::*;

fn observed() -> HomeBackupOrphanObservation {
    HomeBackupOrphanObservation {
        backup_id: "hb-sandbox-fixture-1".into(),
        sandbox_id: "sandbox-fixture".into(),
        home_source_id: "source-fixture".into(),
        home_identity: "home-fixture".into(),
        snapshot_name: "capture-1".into(),
        snapshot_class: "retain-fixture".into(),
        snapshot_generation: "object-generation-fixture".into(),
        source_reference: Some("handle-fixture".into()),
        creation_time: Some("2026-11-16T00:00:00Z".into()),
        ready: true,
    }
}

const NOW: &str = "2026-11-17T00:00:00Z";

#[test]
fn lost_catalog_record_is_imported_but_existing_record_is_preserved() {
    assert_eq!(
        decide_orphan(&observed(), None, false, NOW).unwrap(),
        HomeBackupOrphanDecision::ImportByCreationTime
    );
    let known = backup(HomeBackupState::Ready);
    assert_eq!(
        decide_orphan(&observed(), Some(&known), false, NOW).unwrap(),
        HomeBackupOrphanDecision::Catalogued
    );
}

#[test]
fn tombstone_and_existing_deletion_never_reimport_a_copy() {
    assert_eq!(
        decide_orphan(&observed(), None, true, NOW).unwrap(),
        HomeBackupOrphanDecision::AdmitDeletion
    );
    for state in [HomeBackupState::Expiring, HomeBackupState::Deleted] {
        let mut known = backup(HomeBackupState::Ready);
        known.state = state;
        if state == HomeBackupState::Deleted {
            known.erasure = Some(HomeBackupErasure {
                erased_at: NOW.into(),
                receipt: "independent-absence-fixture".into(),
            });
        }
        assert_eq!(
            decide_orphan(&observed(), Some(&known), false, NOW).unwrap(),
            HomeBackupOrphanDecision::AdmitDeletion
        );
    }
}

#[test]
fn incomplete_copy_waits_without_fabricating_evidence_but_tombstone_still_applies() {
    for missing in ["ready", "handle", "creation"] {
        let mut observation = observed();
        match missing {
            "ready" => observation.ready = false,
            "handle" => observation.source_reference = None,
            _ => observation.creation_time = None,
        }
        assert_eq!(
            decide_orphan(&observation, None, false, NOW).unwrap(),
            HomeBackupOrphanDecision::AwaitEvidence
        );
        assert_eq!(
            decide_orphan(&observation, None, true, NOW).unwrap(),
            HomeBackupOrphanDecision::AdmitDeletion
        );
    }
}

#[test]
fn identity_or_capture_boundary_mismatch_rejects_even_a_tombstoned_target() {
    for field in [
        "id", "sandbox", "home", "source", "name", "class", "handle", "creation",
    ] {
        let mut observation = observed();
        match field {
            "id" => observation.backup_id.push_str("-other"),
            "sandbox" => observation.sandbox_id.push_str("-other"),
            "home" => observation.home_identity.push_str("-other"),
            "source" => observation.home_source_id.push_str("-other"),
            "name" => observation.snapshot_name.push_str("-other"),
            "class" => observation.snapshot_class.push_str("-other"),
            "handle" => observation.source_reference = Some("other-copy-fixture".into()),
            _ => observation.creation_time = Some("2026-11-16T00:00:01Z".into()),
        }
        let known = backup(HomeBackupState::Ready);
        assert_eq!(
            decide_orphan(&observation, Some(&known), true, NOW),
            Err(HomeBackupError::Conflict),
            "{field}"
        );
    }
}

#[test]
fn malformed_or_future_inventory_evidence_is_not_imported() {
    for field in ["id", "generation", "timestamp", "future", "empty_handle"] {
        let mut observation = observed();
        match field {
            "id" => observation.backup_id.clear(),
            "generation" => observation.snapshot_generation = " invalid ".into(),
            "timestamp" => observation.creation_time = Some("2026-11-16".into()),
            "future" => observation.creation_time = Some("2026-11-18T00:00:00Z".into()),
            _ => observation.source_reference = Some(String::new()),
        }
        assert!(
            decide_orphan(&observation, None, false, NOW).is_err(),
            "{field}"
        );
    }
    assert!(decide_orphan(&observed(), None, false, "invalid").is_err());
}
