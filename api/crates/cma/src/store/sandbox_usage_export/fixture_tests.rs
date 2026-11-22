use super::*;
use layer2_core::sandbox_usage::{Allocation, UsageEvent, UsageEventKind};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

const MANIFEST_SHA256: &str = "2f189190dd24595d0b261a56030be38d2328df8c285d35fec08c3e3293948797";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cma_usage_v1")
}

pub(super) fn assert_wire_event(value: &Value) {
    static VALIDATOR: std::sync::OnceLock<jsonschema::Validator> = std::sync::OnceLock::new();
    let validator = VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/cma_usage_v1/schema.json"
        ))
        .unwrap();
        jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap()
    });
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "complete producer event violates N1 schema: {errors:?}"
    );
}

#[test]
fn shared_release_manifest_pins_every_fixture_and_canonical_digest() {
    let bytes = fs::read(root().join("manifest.json")).unwrap();
    assert_eq!(records::hash(&bytes), MANIFEST_SHA256);
    let manifest: Value = serde_json::from_slice(&bytes).unwrap();
    let files = manifest["files"].as_object().unwrap();
    let actual: BTreeSet<_> = fs::read_dir(root())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != "manifest.json")
        .collect();
    assert_eq!(actual, files.keys().cloned().collect());
    for (name, hash) in files {
        assert_eq!(
            records::hash(&fs::read(root().join(name)).unwrap()),
            hash.as_str().unwrap()
        );
    }
    assert_eq!(
        records::hash(&serde_json_canonicalizer::to_vec(files).unwrap()),
        manifest["bundle_sha256"]
    );
    for entry in manifest["fixtures"].as_array().unwrap() {
        let value: Value = serde_json::from_slice(
            &fs::read(root().join(entry["file"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
        let canonical = fs::read(root().join(entry["canonical_file"].as_str().unwrap())).unwrap();
        assert_eq!(serde_json_canonicalizer::to_vec(&value).unwrap(), canonical);
        assert_eq!(records::hash(&canonical), entry["canonical_sha256"]);
        if entry["kind"] == "event" {
            assert_wire_event(&value);
        }
    }
}

fn golden_capture() -> (Value, records::Captured) {
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/cma_usage_v1/allocated.json"
    ))
    .unwrap();
    let fact = &golden["facts"][0];
    let seconds = |field: &str| fact[field].as_str().unwrap().parse::<u64>().unwrap() / 1000;
    let allocation = Allocation {
        running: true,
        cpu_millis: 2000,
        memory_mib: 1024,
        disk_gib: 10,
    };
    let source = UsageEvent {
        sandbox_id: layer2_core::SandboxId("sbx_synthetic".into()),
        owner: layer2_core::SubjectId::new("owner-example"),
        revision: 42,
        source_seq: 1,
        kind: UsageEventKind::Checkpoint,
        started_at: seconds("start_ms"),
        ended_at: seconds("end_ms"),
        quantity: allocation.usage(60).unwrap(),
        next_allocation: allocation.clone(),
        allocation,
        status: "active".into(),
    };
    let capture = records::Captured {
        stream_id: golden["stream_id"].as_str().unwrap().into(),
        previous_revision: Some(41),
        source_gap: false,
        tracking_started_at: source.started_at,
        complete_lifecycle: false,
        resource: records::ResourceSnapshot {
            runtime_id: Some("runtime-example".into()),
            runtime_generation: Some(7),
            home_id: Some("home-example".into()),
            home_generation: Some("2".into()),
        },
        source,
    };
    (golden, capture)
}

#[test]
fn allocated_minute_matches_the_shared_golden_fact_exactly() {
    let (golden, capture) = golden_capture();
    let fact = &golden["facts"][0];
    let batch =
        layer2_core::sandbox_usage_evidence::project(&capture.source, capture.source.started_at)
            .unwrap();
    let projected =
        payload::fact_value(&capture, &batch.facts[0], fact["fact_id"].as_str().unwrap()).unwrap();
    assert_eq!(&projected, fact);
    assert_eq!(
        serde_json_canonicalizer::to_vec(&projected).unwrap(),
        serde_json_canonicalizer::to_vec(fact).unwrap()
    );
}

#[test]
fn positive_usage_requires_each_runtime_and_home_binding_component() {
    let (golden, capture) = golden_capture();
    let batch =
        layer2_core::sandbox_usage_evidence::project(&capture.source, capture.source.started_at)
            .unwrap();
    let id = golden["facts"][0]["fact_id"].as_str().unwrap();
    for resource in [
        records::ResourceSnapshot {
            runtime_id: None,
            ..capture.resource.clone()
        },
        records::ResourceSnapshot {
            runtime_generation: None,
            ..capture.resource.clone()
        },
        records::ResourceSnapshot {
            home_id: None,
            ..capture.resource.clone()
        },
        records::ResourceSnapshot {
            home_generation: None,
            ..capture.resource.clone()
        },
    ] {
        let incomplete = records::Captured {
            resource,
            ..capture.clone()
        };
        assert!(matches!(
            payload::fact_value(&incomplete, &batch.facts[0], id),
            Err(SandboxStoreError::Unavailable(_))
        ));
    }
}

#[test]
fn inactive_incomplete_bindings_map_to_null_pairs_without_changing_source_evidence() {
    for mask in 0..16 {
        let (mut event, mut capture) = golden_capture();
        capture.source.allocation = Allocation::default();
        capture.source.next_allocation = Allocation::default();
        capture.source.quantity = capture.source.allocation.usage(60).unwrap();
        capture.resource = records::ResourceSnapshot {
            runtime_id: (mask & 1 != 0).then(|| "runtime-example".into()),
            runtime_generation: (mask & 2 != 0).then_some(7),
            home_id: (mask & 4 != 0).then(|| "home-example".into()),
            home_generation: (mask & 8 != 0).then(|| "opaque-home-generation".into()),
        };
        let source_bytes = serde_json::to_vec(&capture).unwrap();
        let batch = layer2_core::sandbox_usage_evidence::project(
            &capture.source,
            capture.source.started_at,
        )
        .unwrap();
        let id = event["facts"][0]["fact_id"].as_str().unwrap();
        let fact = payload::fact_value(&capture, &batch.facts[0], id).unwrap();
        assert_eq!(
            fact["resource"],
            serde_json::json!({
                "runtime_id": (mask & 3 == 3).then_some("runtime-example"),
                "runtime_generation": (mask & 3 == 3).then_some("7"),
                "home_id": (mask & 12 == 12).then_some("home-example"),
                "home_generation": (mask & 12 == 12).then_some("opaque-home-generation"),
            })
        );
        event["facts"][0] = fact;
        assert_wire_event(&event);
        assert_eq!(serde_json::to_vec(&capture).unwrap(), source_bytes);
    }
}
