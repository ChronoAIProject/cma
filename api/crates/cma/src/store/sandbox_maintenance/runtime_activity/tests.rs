use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;
use crate::store::sandboxes::sandbox_document;
use mongodb::bson::Bson;

#[test]
fn projected_authority_preserves_absence_and_refuses_inconsistent_envelopes() {
    let sandbox = active_sandbox("sbx_projection");
    let root = sandbox_document(&sandbox, 0).unwrap();
    let first = records::aggregate(&root, &sandbox.id, false).unwrap();
    assert!(!first.active);
    assert_eq!(first.counters[&RuntimeActivityCounter::Desktop], None);
    let mut changed = root.clone();
    changed.insert("desktop_activity_fence", 0_i64);
    let zero = records::aggregate(&changed, &sandbox.id, false).unwrap();
    assert_ne!(first, zero);
    assert_eq!(zero.counters[&RuntimeActivityCounter::Desktop], Some(0));
    for (key, value) in [
        ("revision", Bson::Int64(2)),
        ("work_pending", Bson::Boolean(false)),
        (
            "work_lease_expires_at",
            Bson::String("2026-11-15T00:00:00Z".into()),
        ),
        ("document_schema_version", Bson::Int64(99)),
        ("desktop_activity_fence", Bson::Int64(-1)),
    ] {
        let mut invalid = root.clone();
        invalid.insert(key, value);
        assert!(
            records::aggregate(&invalid, &sandbox.id, false).is_err(),
            "{key}"
        );
    }
}

#[test]
fn parked_update_requires_failed_exact_target_and_no_work_or_lease() {
    let sandbox = active_sandbox("sbx_parked_projection");
    let mut root = sandbox_document(&sandbox, 0).unwrap();
    root.insert("status", "rotating");
    root.insert("work_pending", false);
    let record = root.get_document_mut("record").unwrap();
    record.insert("status", "rotating");
    record
        .get_document_mut("work")
        .unwrap()
        .insert("step", doc! {"kind":"none"});
    let lifecycle = record.get_document_mut("lifecycle").unwrap();
    lifecycle.insert("operation", doc! {"purpose":"image_rotation", "phase":"runtime_provisioning", "target_runtime_version":"image-next"});
    lifecycle.insert("runtime_update", doc! {"request":{"target_version":"image-next", "started_at":"2026-11-15T00:00:00Z", "failure":{"class":"runtime_update_failed"}}});
    assert!(
        !records::aggregate(&root, &sandbox.id, false)
            .unwrap()
            .active
    );
    let mut changed = root.clone();
    changed
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("lifecycle")
        .unwrap()
        .get_document_mut("operation")
        .unwrap()
        .insert("target_runtime_version", "other");
    assert!(
        records::aggregate(&changed, &sandbox.id, false)
            .unwrap()
            .active
    );
    let mut changed = root.clone();
    changed
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("lifecycle")
        .unwrap()
        .get_document_mut("runtime_update")
        .unwrap()
        .get_document_mut("request")
        .unwrap()
        .remove("failure");
    assert!(
        records::aggregate(&changed, &sandbox.id, false)
            .unwrap()
            .active
    );
    let mut changed = root;
    changed.insert("work_pending", true);
    changed
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("work")
        .unwrap()
        .insert("step", doc! {"kind":"run_lifecycle_operation"});
    assert!(
        records::aggregate(&changed, &sandbox.id, false)
            .unwrap()
            .active
    );
}

#[test]
fn observation_projections_never_select_prompt_or_credential_payloads() {
    for projection in [
        projection::sandbox(),
        projection::workspace(),
        projection::agent(),
        projection::command(),
        projection::log_tail(),
        projection::head(),
        crate::store::sandbox_maintenance::evidence::sessions::projection(),
    ] {
        for key in projection.keys() {
            assert_ne!(key, "record");
            assert_ne!(key, "owner");
            assert!(
                !key.contains("prompt")
                    && !key.contains("credential")
                    && !key.contains("payload")
                    && !key.contains("title"),
                "{key}"
            );
        }
    }
    assert!(!projection::agent().contains_key("record.conversation.queued_messages"));
    assert!(!projection::log_tail().contains_key("record.kind"));
    assert!(projection::workspace().contains_key("record.ready_at"));
}

#[test]
fn terminal_command_dispatch_is_history_but_lease_or_pending_status_is_active() {
    let base = doc! {"_id":"cmd_history", "document_schema_version":1_i64,
    "agent_id":"agt_history", "agent_order":1_i64, "revision":1_i64,
    "status":"completed", "record":{"revision":1_i64, "status":"completed",
        "receipt":{"envelope":{"command_id":"cmd_history", "agent_id":"agt_history"}, "agent_order":1_i64},
        "work":{"dispatch":{"delivery":"execution_observed"}}}};
    assert!(!records::command(&base).unwrap().active);
    for status in ["queued", "in_progress", "waiting_action"] {
        let mut row = base.clone();
        row.insert("status", status);
        row.get_document_mut("record")
            .unwrap()
            .insert("status", status);
        assert!(records::command(&row).unwrap().active);
    }
    let mut lease = base.clone();
    lease
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("work")
        .unwrap()
        .insert("lease", doc! {"holder":"fixture"});
    assert!(records::command(&lease).unwrap().active);
    let mut invalid = base;
    invalid
        .get_document_mut("record")
        .unwrap()
        .insert("status", "queued");
    assert!(records::command(&invalid).is_err());
}
