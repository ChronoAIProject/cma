//! Canonical erased aggregates remain readable without storing private drafts.
use crate::store::{agent_profiles, sandbox_profiles, workspace_profiles};
use layer2_core::profiles::{store::ProfileStoreError, WorkspaceLocation};
use layer2_core::DELETED_ACCOUNT_SUBJECT;
use mongodb::bson::{doc, Bson, DateTime, Document};

const NOW: &str = "2026-11-14T00:00:00Z";

fn retained(kind: &str) -> Document {
    let id = format!("{kind}-retained");
    let mut record = doc! {
        "id":&id,"owner":DELETED_ACCOUNT_SUBJECT,"name":"","description":"",
        "visibility":"private","uses_count":3_i64,"draft_version":7_i64,
        "created_at":NOW,"updated_at":NOW,"retired_at":NOW,
    };
    if kind == "sandbox" {
        record.insert(
            "availability",
            doc! {
                "warmup_enabled":false,"warmup_count":3_i32,"always_keep_alive":true,
            },
        );
    }
    doc! {
        "_id":id,"document_schema_version":crate::store::schema::DOCUMENT_SCHEMA_VERSION,
        "owner":DELETED_ACCOUNT_SUBJECT,"name":"","description":"","visibility":"private",
        "kind":kind,"uses_count":3_i64,"draft_version":7_i64,"updated_at":NOW,"retired_at":NOW,
        "last_revision":2_i64,"latest_published_at":NOW,"record":record,
        "account_erasure_retained":true,"account_erasure_fenced":true,
        "account_erasure_fenced_at":DateTime::from_millis(0),
    }
}

fn read(kind: &str, document: &Document) -> Result<(), ProfileStoreError> {
    match kind {
        "agent" => agent_profiles::read_profile(document).map(|_| ()),
        "workspace" => workspace_profiles::read_profile(document).map(|_| ()),
        "sandbox" => sandbox_profiles::read_profile(document).map(|_| ()),
        _ => unreachable!(),
    }
}

#[test]
fn retained_profile_readers_hydrate_only_in_memory_and_preserve_availability() {
    let agent_row = retained("agent");
    let agent = agent_profiles::read_profile(&agent_row).unwrap();
    assert_eq!(agent.owner.as_str(), DELETED_ACCOUNT_SUBJECT);
    assert!(agent.name.is_empty() && agent.description.is_empty());
    assert_eq!(agent.retired_at.as_deref(), Some(NOW));
    assert!(agent.draft.model.is_none());
    assert!(agent.draft.skills.is_empty() && agent.draft.system_prompt.is_empty());

    let workspace_row = retained("workspace");
    let workspace = workspace_profiles::read_profile(&workspace_row).unwrap();
    assert!(matches!(
        workspace.draft.location,
        WorkspaceLocation::Directory { .. }
    ));
    assert!(workspace.draft.agent_profiles.is_empty() && workspace.draft.setup_commands.is_empty());

    let sandbox_row = retained("sandbox");
    let sandbox = sandbox_profiles::read_profile(&sandbox_row).unwrap();
    assert!(sandbox.availability.always_keep_alive);
    assert!(!sandbox.availability.warmup_enabled);
    assert_eq!(sandbox.availability.warmup_count, 3);
    assert!(sandbox.draft.workspace_profiles.is_empty());
    assert!(sandbox.draft.environment_variables.is_empty());
    assert!(sandbox.draft.software_install_commands.is_empty());
    assert!(sandbox.draft.runtime_image.is_none());
    for row in [agent_row, workspace_row, sandbox_row] {
        assert!(!row.get_document("record").unwrap().contains_key("draft"));
    }
}

#[test]
fn ordinary_profile_missing_a_draft_is_corrupt() {
    for kind in ["agent", "workspace", "sandbox"] {
        let mut row = retained(kind);
        for field in [
            "account_erasure_retained",
            "account_erasure_fenced",
            "account_erasure_fenced_at",
        ] {
            row.remove(field);
        }
        row.insert("owner", "ordinary-owner");
        row.insert("name", "Ordinary profile");
        row.insert("retired_at", Bson::Null);
        let record = row.get_document_mut("record").unwrap();
        record.insert("owner", "ordinary-owner");
        record.insert("name", "Ordinary profile");
        record.remove("retired_at");
        assert!(matches!(
            read(kind, &row),
            Err(ProfileStoreError::Corrupt(_))
        ));
    }
}

#[test]
fn partial_or_forged_retention_markers_cannot_hydrate_a_draft() {
    for kind in ["agent", "workspace", "sandbox"] {
        let original = retained(kind);
        let mut malformed = vec![];
        for field in [
            "account_erasure_retained",
            "account_erasure_fenced",
            "account_erasure_fenced_at",
        ] {
            let mut row = original.clone();
            row.remove(field);
            malformed.push(row);
        }
        for (field, value) in [
            ("account_erasure_retained", Bson::Boolean(false)),
            ("account_erasure_fenced_at", Bson::String(NOW.into())),
            ("owner", Bson::String("ordinary-owner".into())),
            ("name", Bson::String("private authoring name".into())),
            ("visibility", Bson::String("public".into())),
            ("retired_at", Bson::String("unverified".into())),
            ("kind", Bson::String("unknown".into())),
        ] {
            let mut row = original.clone();
            row.insert(field, value);
            malformed.push(row);
        }
        for (field, value) in [
            ("owner", Bson::String("ordinary-owner".into())),
            ("description", Bson::String("private description".into())),
            ("retired_at", Bson::String("2026-11-15T00:00:00Z".into())),
            ("draft", Bson::Document(doc! {})),
            ("forked_from", Bson::Null),
        ] {
            let mut row = original.clone();
            row.get_document_mut("record").unwrap().insert(field, value);
            malformed.push(row);
        }
        for row in malformed {
            assert!(
                matches!(read(kind, &row), Err(ProfileStoreError::Corrupt(_))),
                "{row:?}"
            );
        }
    }
}
