use super::*;
use layer2_core::sandbox_usage::{Allocation, UsageEvent, UsageEventKind};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::PathBuf};

const MANIFEST_SHA256: &str = "5aa9606080eef4123e5314f0eac412f2c1bc018eb83aff172e01bfbbc14ac033";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cma_usage_v1")
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
    }
}

#[test]
fn allocated_minute_matches_the_shared_golden_fact_exactly() {
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
