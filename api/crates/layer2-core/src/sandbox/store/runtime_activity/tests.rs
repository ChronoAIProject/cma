use super::*;
use std::collections::BTreeMap;

fn proof() -> RuntimeActivityProof {
    let source = RuntimeActivitySourceContext {
        source_id: "fixture".into(),
        adapter_version: "1".into(),
        policy_id: "test".into(),
        producer_id: "test".into(),
        database_id: "fixture".into(),
        api_image: "image".into(),
        source_revision: "revision".into(),
        process_incarnation: "process-1".into(),
        serving_processes: 1,
        replica_mode: false,
        clock_uncertainty_ms: 0,
        coverage: RuntimeActivityCoverage::MemoryFixtureV1,
    };
    let first = RuntimeActivitySnapshot {
        source,
        capture_started_at_ms: 100_000,
        capture_completed_at_ms: 100_000,
        watermark: RuntimeActivityWatermark {
            seconds: 100,
            increment: 1,
        },
        storage_incarnation: "mongo-process-1".into(),
        desktop: RuntimeActivityDesktop {
            process_incarnation: "process-1".into(),
            tracking_incarnation: "tracker-1".into(),
            activity_epoch: 0,
            opening_or_open: 0,
        },
        vector: RuntimeActivityVector {
            sandbox_id: SandboxId("sandbox".into()),
            sandbox: RuntimeActivityRecord {
                id: "sandbox".into(),
                metadata_sha256: "a".repeat(64),
                revision: Some(1),
                last_seq: Some(0),
                counters: BTreeMap::new(),
                active: false,
            },
            workspaces: vec![],
            agents: vec![],
            command_orders: vec![],
            commands: vec![],
            log_heads: vec![],
            log_tails: vec![],
            desktop_leases: vec![],
        },
    };
    let mut last = first.clone();
    last.capture_started_at_ms = 1_000_000;
    last.capture_completed_at_ms = 1_000_000;
    last.watermark.seconds = 1000;
    RuntimeActivityProof {
        schema_version: 1,
        initial: first,
        final_observation: last,
    }
}

#[test]
fn proof_preserves_original_900_seconds_and_never_refreshes_at_admission() {
    let value = proof();
    value.validate_at(1_000_000).unwrap();
    value.validate_at(1_030_000).unwrap();
    assert!(value.validate_at(1_030_001).is_err());
    assert!(value.validate_at(999_999).is_err());
    let encoded = serde_json::to_vec(&value).unwrap();
    let decoded: RuntimeActivityProof = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, value);
    assert_eq!(decoded.final_observation.capture_completed_at_ms, 1_000_000);
}

#[test]
fn short_uncertain_future_gapped_or_unqualified_coverage_refuses() {
    for case in 0..9 {
        let mut value = proof();
        match case {
            0 => value.initial.capture_completed_at_ms += 1,
            1 => {
                value.initial.source.clock_uncertainty_ms = 1;
                value.final_observation.source.clock_uncertainty_ms = 1;
            }
            2 => value.final_observation.watermark.seconds = 99,
            3 => value.final_observation.source.producer_id = "replacement".into(),
            4 => value.final_observation.source.replica_mode = true,
            5 => value.final_observation.source.serving_processes = 2,
            6 => value.final_observation.source.coverage = RuntimeActivityCoverage::Unqualified,
            7 => value.final_observation.storage_incarnation = "replacement-db".into(),
            8 => value.initial.capture_completed_at_ms += 5_001,
            _ => unreachable!(),
        }
        assert!(value.validate_at(1_000_000).is_err(), "case {case}");
    }
}

#[test]
fn activity_membership_and_every_source_counter_drift_refuse() {
    for case in 0..7 {
        let mut value = proof();
        match case {
            0 => value.final_observation.vector.sandbox.active = true,
            1 => value.final_observation.desktop.opening_or_open = 1,
            2 => value.final_observation.desktop.activity_epoch = 1,
            3 => {
                value.final_observation.desktop.tracking_incarnation =
                    "evicted-and-recreated".into()
            }
            4 => {
                value
                    .final_observation
                    .vector
                    .sandbox
                    .counters
                    .insert(RuntimeActivityCounter::Desktop, Some(0));
            }
            5 => value
                .final_observation
                .vector
                .workspaces
                .push(RuntimeActivityRecord {
                    id: "created".into(),
                    metadata_sha256: "b".repeat(64),
                    revision: Some(1),
                    last_seq: None,
                    counters: BTreeMap::new(),
                    active: false,
                }),
            6 => value.final_observation.vector.sandbox.metadata_sha256 = "c".repeat(64),
            _ => unreachable!(),
        }
        assert!(value.validate_at(1_000_000).is_err(), "case {case}");
    }
}

#[test]
fn admission_compares_exact_final_context_without_rebasing_counter() {
    let value = proof();
    let mut current = value.final_observation.clone();
    current.capture_started_at_ms += 10;
    current.capture_completed_at_ms += 10;
    value.compare_current(&current, 1_000_010).unwrap();
    current.vector.sandbox.revision = Some(2);
    assert!(value.compare_current(&current, 1_000_010).is_err());
    assert_eq!(value.final_observation.vector.sandbox.revision, Some(1));
}

#[test]
fn incomplete_log_context_and_duplicate_or_overflow_membership_refuse() {
    let mut value = proof();
    let agent = RuntimeActivityRecord {
        id: "agent".into(),
        metadata_sha256: "b".repeat(64),
        revision: Some(1),
        last_seq: Some(1),
        counters: BTreeMap::new(),
        active: false,
    };
    value.initial.vector.agents.push(agent.clone());
    assert!(value.initial.validate().is_err());
    value.initial.vector.log_tails.push(agent.clone());
    value.initial.validate().unwrap();
    value.initial.vector.agents.push(agent.clone());
    assert!(value.initial.validate().is_err());
    value.initial.vector.agents = vec![agent; RUNTIME_ACTIVITY_MAX_ROWS + 1];
    assert!(value.initial.validate().is_err());
}
