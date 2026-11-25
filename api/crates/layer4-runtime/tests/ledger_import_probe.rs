//! Exercise the offline executable interface without a supervisor or provider.
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-ledger-import-cli-{}", uuid::Uuid::new_v4()));
        for directory in [
            "home",
            "state/home-binding",
            "state/mutation-authority",
            "transfer/cma-checkpoints",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(
            root.join("state/home-binding/binding.json"),
            serde_json::to_vec(&serde_json::json!({
                "version":2,"sandbox_id":"ledger-cli-fixture","home_identity":"a".repeat(43),
                "runtime_generation":7,"restore":null,
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("state/mutation-authority/.authority-1.tmp"),
            b"unpublished authority",
        )
        .unwrap();
        std::fs::write(root.join("home/wanted-file"), b"preserved home bytes").unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str], valid_config: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_layer4-runtime"));
        command
            .env_clear()
            .args(args)
            .env("CHRONO_RUNTIME_HOME", self.0.join("home"))
            .env("CHRONO_RUNTIME_STATE_ROOT", self.0.join("state"))
            .env("CHRONO_RUNTIME_GENERATION", "7")
            .env("CHRONO_RUNTIME_HOME_IDENTITY", "a".repeat(43))
            .env("TMPDIR", self.0.join("transfer"));
        if valid_config {
            command.env("CHRONO_SANDBOX_ID", "ledger-cli-fixture");
        }
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().unwrap()
    }
    fn success(&self, args: &[&str]) -> serde_json::Value {
        let result = self.run(args, true);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(!text.contains("ledger-cli-fixture"));
        assert!(!text.contains(self.0.to_str().unwrap()));
        assert!(!text.contains("unpublished authority"));
        serde_json::from_str(&text).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn retirement_archive_cli_builds_only_a_detached_pending_copy() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let plan = fixture.0.join("retirement-plan");
    let report = fixture.success(&["--plan-ledger-v1", plan.to_str().unwrap()]);
    let plan_hash = report["plan_sha256"].as_str().unwrap();
    let observation = fixture.0.join("observation.json");
    let snapshot = layer3_adapter::runtime_ledger_retirement::RuntimeLedgerRetirementSnapshot {
        format: layer3_adapter::runtime_ledger_retirement::LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
        sandbox_id: "ledger-cli-fixture".into(),
        home_identity: "a".repeat(43),
        generation: 7,
        sandbox_revision: 1,
        observed_at: "2026-11-11T00:00:00Z".into(),
        source_sha256: "b".repeat(64),
        agents_observed: 0,
        worktrees_observed: 0,
        deleted_agents: vec![],
        removed_worktrees: vec![],
    };
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    let snapshot_hash = format!("{:x}", Sha256::digest(&bytes));
    std::fs::write(&observation, bytes).unwrap();
    let output = fixture.0.join("retirement-archive");
    let args = [
        "--stage-ledger-retirement",
        plan.to_str().unwrap(),
        plan_hash,
        observation.to_str().unwrap(),
        &snapshot_hash,
        output.to_str().unwrap(),
    ];
    let refused = fixture.run(&args, false);
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert!(!output.exists());
    let archived = fixture.success(&args);
    assert_eq!(archived["admission"], "retirement_held");
    assert_eq!(archived["archived_records"], 0);
    assert_eq!(archived["source_deactivated"], false);
    let verified = fixture.success(&[
        "--verify-ledger-retirement",
        output.to_str().unwrap(),
        archived["ready_sha256"].as_str().unwrap(),
    ]);
    assert_eq!(verified["admission"], "retirement_held");
    assert_eq!(verified["archived_records"], 0);
    assert!(!fixture
        .run(
            &[
                "--verify-ledger-retirement",
                output.to_str().unwrap(),
                &"f".repeat(64)
            ],
            true
        )
        .status
        .success());
    assert!(!fixture.run(&args, true).status.success());
    assert!(!fixture
        .0
        .join("state/mutation-retirement-v1.current.json")
        .exists());
    assert_eq!(
        std::fs::read(fixture.0.join("state/mutation-authority/.authority-1.tmp")).unwrap(),
        b"unpublished authority"
    );
    assert_eq!(
        std::fs::read(fixture.0.join("home/wanted-file")).unwrap(),
        b"preserved home bytes"
    );
}

#[test]
fn ledger_import_cli_cutover_refuses_before_configuration_or_source_access() {
    let fixture = Fixture::new();
    for configured in [true, false] {
        for args in [
            vec!["--cutover-ledger-v1"],
            vec!["--cutover-ledger-v1", "/unused", "unused"],
        ] {
            let result = fixture.run(&args, configured);
            assert_eq!(result.status.code(), Some(2));
            assert!(result.stdout.is_empty());
            let message = String::from_utf8(result.stderr).unwrap();
            assert!(message.contains("mutation_ledger_import_held"));
            assert!(message.contains("#1461"));
            assert!(!message.contains("configuration is invalid"));
            assert!(!message.contains(fixture.0.to_str().unwrap()));
        }
    }
    assert_eq!(
        std::fs::read(fixture.0.join("home/wanted-file")).unwrap(),
        b"preserved home bytes"
    );
    assert_eq!(
        std::fs::read(fixture.0.join("state/mutation-authority/.authority-1.tmp")).unwrap(),
        b"unpublished authority"
    );
    assert!(!fixture
        .0
        .join("state/mutation-ledger-v2.current.json")
        .exists());
}

#[test]
fn ledger_import_cli_creates_only_a_detached_held_artifact_and_refuses_replays() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.success(&["--inspect-ledger-v1"])["writer_exclusion"],
        "unproved"
    );
    assert_eq!(
        fixture.success(&["--validate-ledger-v1"])["semantic_validation"],
        "mutation_histories_valid"
    );
    let plan = fixture.0.join("plan");
    let output = fixture.0.join("staged");
    let planned = fixture.success(&["--plan-ledger-v1", plan.to_str().unwrap()]);
    let digest = planned["plan_sha256"].as_str().unwrap();
    fixture.success(&["--verify-ledger-plan", plan.to_str().unwrap(), digest]);
    let staged = fixture.success(&[
        "--stage-ledger-v1",
        plan.to_str().unwrap(),
        digest,
        output.to_str().unwrap(),
    ]);
    assert_eq!(staged["admission"], "import_held");
    assert_eq!(staged["operations"], 0);
    let database = output.join("database/ledger.sqlite3");
    let before = std::fs::read(&database).unwrap();
    assert!(!fixture
        .run(&["--plan-ledger-v1", plan.to_str().unwrap()], true)
        .status
        .success());
    assert!(!fixture
        .run(
            &[
                "--stage-ledger-v1",
                plan.to_str().unwrap(),
                digest,
                output.to_str().unwrap()
            ],
            true
        )
        .status
        .success());
    assert_eq!(before, std::fs::read(database).unwrap());
    assert!(!fixture
        .run(
            &[
                "--verify-ledger-plan",
                plan.to_str().unwrap(),
                &"b".repeat(64)
            ],
            true
        )
        .status
        .success());
    assert_eq!(
        std::fs::read(fixture.0.join("home/wanted-file")).unwrap(),
        b"preserved home bytes"
    );
    assert_eq!(
        std::fs::read(fixture.0.join("state/mutation-authority/.authority-1.tmp")).unwrap(),
        b"unpublished authority"
    );
    assert!(!fixture
        .0
        .join("state/mutation-ledger-v2.current.json")
        .exists());
}

#[test]
fn ledger_import_cli_configuration_failures_are_content_free_and_never_start_recovery() {
    let fixture = Fixture::new();
    for args in [
        vec!["--inspect-ledger-v1"],
        vec!["--validate-ledger-v1"],
        vec!["--plan-ledger-v1", "/unused"],
        vec!["--verify-ledger-plan", "/unused", "unused"],
        vec!["--stage-ledger-v1", "/unused", "unused", "/unused"],
    ] {
        let result = fixture.run(&args, false);
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        let message = String::from_utf8(result.stderr).unwrap();
        assert!(message.contains("runtime ledger inspection configuration is invalid"));
        assert!(!message.contains(fixture.0.to_str().unwrap()));
    }
    assert!(!fixture
        .0
        .join("state/mutation-ledger-v2.current.json")
        .exists());
}

#[test]
fn evidence_export_requires_an_explicit_destination_and_valid_configuration() {
    let fixture = Fixture::new();
    for args in [
        vec!["--export-ledger-v1-evidence"],
        vec!["--export-ledger-v1-evidence", "one", "two"],
    ] {
        let result = fixture.run(&args, false);
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8(result.stderr)
            .unwrap()
            .contains("ledger_evidence_arguments_invalid"));
    }
    let destination = fixture.0.join("evidence");
    let result = fixture.run(
        &["--export-ledger-v1-evidence", destination.to_str().unwrap()],
        false,
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8(result.stderr)
        .unwrap()
        .contains("ledger_evidence_configuration_invalid"));
    assert!(!destination.exists());
}

#[test]
fn evidence_export_refuses_writable_sources_without_output_or_recovery() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("evidence");
    let result = fixture.run(
        &["--export-ledger-v1-evidence", destination.to_str().unwrap()],
        true,
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    let message = String::from_utf8(result.stderr).unwrap();
    assert!(message.contains("ledger_evidence_readonly_required"));
    assert!(!message.contains(fixture.0.to_str().unwrap()));
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read(fixture.0.join("home/wanted-file")).unwrap(),
        b"preserved home bytes"
    );
    assert_eq!(
        std::fs::read(fixture.0.join("state/mutation-authority/.authority-1.tmp")).unwrap(),
        b"unpublished authority"
    );
}
