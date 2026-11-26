use super::super::evidence_export::{self as export, projection::project};
use super::*;

#[test]
fn export_decodes_all_states_and_removes_payload_and_lease_authority() {
    for state in [
        HomeBackupState::Requested,
        HomeBackupState::Creating,
        HomeBackupState::Ready,
        HomeBackupState::Failed,
        HomeBackupState::Expiring,
        HomeBackupState::Deleted,
    ] {
        let mut value = standalone_backup(state);
        value.lease = Some(HomeBackupLease {
            holder: format!("lease-holder-sentinel:{}", uuid::Uuid::new_v4()),
            token: uuid::Uuid::new_v4().to_string(),
            fence: 1,
            expires_at: until(),
        });
        value.next_fence = 2;
        let out = project(BACKUPS_COLLECTION, backup_document(&value).unwrap()).unwrap();
        let bytes = out.to_string();
        assert!(!bytes.contains("lease-holder-sentinel"));
        assert!(!bytes.contains(&value.lease.as_ref().unwrap().token));
        assert_eq!(out["authority"]["lease_present"], true);
        assert_eq!(out["source"]["home_source_id"], value.source.home_source_id);
        assert_eq!(
            out["cut"]["source_creation_time"],
            serde_json::json!(value.cut.source_creation_time)
        );
    }
    let mut parent = discovery_tests::retained("legacy", false);
    parent.owner = layer2_core::SubjectId::new("export-private-owner-sentinel");
    parent.name = "unrelated-sandbox-payload-sentinel".into();
    let row = crate::store::sandboxes::sandbox_document(&parent, 0).unwrap();
    let out = project("sandboxes", row).unwrap();
    assert!(!out
        .to_string()
        .contains("unrelated-sandbox-payload-sentinel"));
    assert!(!out.to_string().contains(parent.owner.as_str()));
    assert_eq!(
        out["home"]["receipt"]["storage_generation"],
        "source-fixture"
    );
}

#[test]
fn export_rejects_invalid_complete_records_and_lossy_numeric_values() {
    for collection in [SCHEDULES_COLLECTION, BACKUPS_COLLECTION] {
        let row = if collection == SCHEDULES_COLLECTION {
            schedule_document(&schedule()).unwrap()
        } else {
            backup_document(&standalone_backup(HomeBackupState::Creating)).unwrap()
        };
        let mut exact = row.clone();
        exact.insert("next_fence", (1_i64 << 53) - 1);
        assert!(project(collection, exact).is_ok());
        let mut rounded = row.clone();
        rounded.insert("next_fence", 1_i64 << 53);
        assert_eq!(
            project(collection, rounded).unwrap_err(),
            "catalog_export_numeric_range"
        );
        let mut future = row.clone();
        future.insert("document_schema_version", 999);
        assert!(project(collection, future).is_err());
        let mut wrong_identity = row.clone();
        wrong_identity.insert("_id", "different-record");
        assert!(project(collection, wrong_identity).is_err());
        let mut unknown = row;
        unknown
            .get_document_mut(if collection == SCHEDULES_COLLECTION {
                "home"
            } else {
                "source"
            })
            .unwrap()
            .insert("future_field", "payload-sentinel");
        assert!(project(collection, unknown).is_err());
    }
    let parent = discovery_tests::retained("legacy", false);
    let mut row = crate::store::sandboxes::sandbox_document(&parent, 0).unwrap();
    row.get_document_mut("record")
        .unwrap()
        .get_document_mut("lifecycle")
        .unwrap()
        .get_document_mut("home")
        .unwrap()
        .insert("future_field", true);
    assert!(project("sandboxes", row).is_err());
}

#[test]
fn export_authority_requires_only_the_exact_read_and_metadata_grants() {
    let role = export::authority::role("fixture");
    let status = |grants| doc! {"authInfo": {"authenticatedUsers": [{"user": "reader"}], "authenticatedUserPrivileges": grants}};
    let grants = role.get_array("privileges").unwrap().clone();
    export::authority::validate(&status(grants.clone()), "fixture").unwrap();
    let mut insufficient = grants.clone();
    insufficient.remove(0);
    assert!(export::authority::validate(&status(insufficient), "fixture").is_err());
    for action in [
        "insert",
        "update",
        "remove",
        "createCollection",
        "createIndex",
        "collMod",
    ] {
        let mut elevated = grants.clone();
        elevated[0]
            .as_document_mut()
            .unwrap()
            .get_array_mut("actions")
            .unwrap()
            .push(action.into());
        assert!(export::authority::validate(&status(elevated), "fixture").is_err());
    }
    let mut cross_database = grants;
    cross_database[0]
        .as_document_mut()
        .unwrap()
        .get_document_mut("resource")
        .unwrap()
        .insert("db", "other");
    assert!(export::authority::validate(&status(cross_database), "fixture").is_err());
}

#[tokio::test]
async fn export_cli_rejects_malformed_arguments_before_startup_or_connections() {
    assert!(!export::cli::is_command(&[]));
    for arguments in [
        vec!["--home-backup-catalog-export"],
        vec!["--home-backup-catalog-export=ignored"],
        vec!["--home-backup-catalog-export", "relative"],
        vec!["--home-backup-catalog-export", "/tmp/new", "extra"],
        vec!["--other", "--home-backup-catalog-export"],
    ] {
        let arguments: Vec<_> = arguments.into_iter().map(str::to_owned).collect();
        assert!(export::cli::is_command(&arguments));
        assert!(export::cli::command(&arguments).await.is_err());
    }
}
