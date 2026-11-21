//! Reviewed additive results; frozen release bytes and all old fields stay exact.
use super::super::{BodyKind, Corpus};
use serde_json::json;

pub(super) fn apply(release: &str, file: &str, corpus: &mut Corpus) -> usize {
    if !matches!(
        (release, file),
        (
            "93d4a199a3d36d47f5a89f9117d2bc735900bf4c",
            "previous-instances.json"
        ) | (
            "4f03c35a80322673ec00df7a3b6b9399015f5a68",
            "current-instances.json"
        )
    ) {
        return 0;
    }
    for (id, scenario, operation, path, status) in [
        (
            "sandbox-create",
            "instances_owner",
            "POST /api/v1/sandboxes",
            "/api/v1/sandboxes",
            201,
        ),
        (
            "sandbox-pause",
            "instances_lifecycle",
            "POST /api/v1/sandboxes/{id}/pause",
            "/api/v1/sandboxes/sbx_route/pause",
            202,
        ),
    ] {
        let mut selected = corpus.cases.iter_mut().filter(|case| case.id == id);
        let case = selected.next().expect("declared retained lifecycle exists");
        assert!(selected.next().is_none());
        assert_eq!(case.scenario, scenario);
        assert_eq!(case.operation, operation);
        assert_eq!(case.request.method, "POST");
        assert_eq!(case.request.path, path);
        assert_eq!(case.response.status, status);
        assert_eq!(case.response.body_kind, BodyKind::Json);
        let mut expected = json!({
            "pause": {"allowed": false, "reason": "not_active"},
            "revive": {"allowed": false, "reason": "not_paused"}
        });
        if id == "sandbox-pause" {
            expected["operation"] = json!({"purpose": "pause", "phase": "capture_and_publish_checkpoint", "started_at": "2026-09-05T00:00:00Z"});
        }
        assert_eq!(case.response.body["lifecycle"], expected);
        case.response.body["lifecycle"]["result"] = json!({
            "state": "in_progress",
            "operation_id": if id == "sandbox-create" { "$sandbox_id:create:$sandbox_created" } else { "sandbox-lifecycle-v1:sbx_route:1" },
            "started_at": "2026-09-05T00:00:00Z",
            "result_due_at": "2026-09-05T00:01:30Z",
            "continuing": true, "actions": ["observe"], "retry_after_seconds": 5
        });
        assert!(case
            .captures
            .insert(
                "lifecycle_due".into(),
                "/lifecycle/result/result_due_at".into()
            )
            .is_none());
    }
    2
}

#[test]
fn retained_addition_preserves_all_frozen_fields_and_refuses_changed_lifecycle() {
    for (release, file) in [
        (
            "93d4a199a3d36d47f5a89f9117d2bc735900bf4c",
            "previous-instances.json",
        ),
        (
            "4f03c35a80322673ec00df7a3b6b9399015f5a68",
            "current-instances.json",
        ),
    ] {
        let bytes = std::fs::read(super::directory().join(file)).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut corpus: Corpus = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(apply(release, file, &mut corpus), 2);
        for case in &mut corpus.cases {
            if matches!(case.id.as_str(), "sandbox-create" | "sandbox-pause") {
                case.response.body["lifecycle"]
                    .as_object_mut()
                    .unwrap()
                    .remove("result")
                    .unwrap();
                case.captures.remove("lifecycle_due").unwrap();
            }
        }
        assert_eq!(serde_json::to_value(&corpus).unwrap(), original);
        corpus
            .cases
            .iter_mut()
            .find(|case| case.id == "sandbox-pause")
            .unwrap()
            .response
            .body["lifecycle"]["pause"]["allowed"] = true.into();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(
                release,
                file,
                &mut corpus
            )))
            .is_err()
        );
    }
}
