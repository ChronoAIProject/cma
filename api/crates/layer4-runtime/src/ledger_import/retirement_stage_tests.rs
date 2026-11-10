use super::retirement_preview::{completed_removal, snapshot, write_snapshot};
use super::semantic::{authority, fixture, journal, remove};
use super::*;
use crate::ledger_import::retirement_stage::{self, Boundary, Input};
use crate::mutation_journal::{Operation, Record};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

struct Inputs {
    plan: PathBuf,
    plan_sha256: String,
    observation: PathBuf,
    observation_sha256: String,
}
impl Inputs {
    fn new(fixture: &Fixture) -> Self {
        let (observation, observation_sha256) = write_snapshot(fixture, &snapshot(fixture));
        let plan = fixture.root.join("plan");
        let summary = plan::create(&fixture.config, &plan).unwrap();
        Self {
            plan,
            plan_sha256: summary["plan_sha256"].as_str().unwrap().into(),
            observation,
            observation_sha256,
        }
    }
    fn input(&self) -> Input<'_> {
        Input {
            plan: &self.plan,
            plan_sha256: &self.plan_sha256,
            observation: &self.observation,
            observation_sha256: &self.observation_sha256,
        }
    }
}
fn database(path: &Path) -> Connection {
    Connection::open_with_flags(
        path.join("database/ledger.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}
fn completed_fixture() -> Fixture {
    let fixture = fixture();
    authority(&fixture.config, 2, &remove());
    journal(&fixture.config, &completed_removal(&fixture));
    fixture
}

#[tokio::test]
async fn archive_preserves_exact_selected_bytes_and_all_live_source_state() {
    let fixture = completed_fixture();
    let record = completed_removal(&fixture);
    let original = serde_json::to_vec_pretty(&record).unwrap();
    let canonical = journal(&fixture.config, &record);
    std::fs::write(&canonical, &original).unwrap();
    let live = Operation::remove_worktree(&layer3_adapter::WorktreeRef::new(
        "worktree:workspace:live:remove:v1",
        "workspace",
        "live",
    ));
    authority(&fixture.config, 2, &live);
    let inputs = Inputs::new(&fixture);
    let before = fixture.scan().unwrap().digest;
    let output = fixture.root.join("retirement");
    let summary = retirement_stage::create(fixture.config.clone(), inputs.input(), &output)
        .await
        .unwrap();
    assert_eq!(summary["archived_records"], 2);
    assert_eq!(summary["source_deactivated"], false);
    assert_eq!(summary["admission"], "retirement_held");
    assert_eq!(summary["writer_exclusion"], "unproved");
    assert_eq!(summary["candidate_resource_groups"], 1);
    assert_eq!(
        summary["ready_sha256"],
        format!(
            "{:x}",
            Sha256::digest(std::fs::read(output.join("ready.json")).unwrap())
        )
    );
    let db = database(&output);
    let archived: Vec<u8> = db
        .query_row(
            "SELECT payload FROM archived_records WHERE kind=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(archived, original);
    let pending: i64 = db
        .query_row(
            "SELECT count(*) FROM retired_groups WHERE complete=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1);
    assert_eq!(fixture.scan().unwrap().digest, before);
    for private in [
        fixture.config.sandbox_id.as_str(),
        fixture.root.to_str().unwrap(),
        "worktree:workspace",
    ] {
        assert!(!summary.to_string().contains(private));
    }
    drop(db);
    let retained = std::fs::read(output.join("database/ledger.sqlite3")).unwrap();
    assert!(
        retirement_stage::create(fixture.config.clone(), inputs.input(), &output)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(output.join("database/ledger.sqlite3")).unwrap(),
        retained
    );
    assert_eq!(fixture.scan().unwrap().digest, before);
}

#[tokio::test]
async fn archived_selection_uses_only_the_pinned_backup_after_live_sources_change() {
    let fixture = completed_fixture();
    let inputs = Inputs::new(&fixture);
    let original = std::fs::read(journal(&fixture.config, &completed_removal(&fixture))).unwrap();
    std::fs::remove_dir_all(fixture.config.state_root.join("runtime-mutations")).unwrap();
    let sentinel = fixture.write("runtime-mutations/unrelated", b"new live state");
    let output = fixture.root.join("retirement");
    retirement_stage::create(fixture.config.clone(), inputs.input(), &output)
        .await
        .unwrap();
    let db = database(&output);
    let archived: Vec<u8> = db
        .query_row(
            "SELECT payload FROM archived_records WHERE kind=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(archived, original);
    assert_eq!(std::fs::read(sentinel).unwrap(), b"new live state");
}

#[tokio::test]
async fn incorrect_or_changed_input_refuses_without_publishing_a_ready_archive() {
    for case in 0..4 {
        let fixture = completed_fixture();
        let mut inputs = Inputs::new(&fixture);
        let before = fixture.scan().unwrap().digest;
        match case {
            0 => inputs.plan_sha256 = "f".repeat(64),
            1 => inputs.observation_sha256 = "f".repeat(64),
            2 => std::fs::write(&inputs.observation, b"changed").unwrap(),
            _ => {
                use std::os::unix::fs::PermissionsExt;
                let source = inputs.plan.join("00000000.record");
                std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(source, b"changed").unwrap();
            }
        }
        let output = fixture.root.join("retirement");
        assert!(
            retirement_stage::create(fixture.config.clone(), inputs.input(), &output)
                .await
                .is_err()
        );
        assert!(!output.join("ready.json").exists());
        assert_eq!(fixture.scan().unwrap().digest, before);
    }
}

#[tokio::test]
async fn interruption_and_output_replacement_cannot_publish_or_touch_other_paths() {
    for case in 0..4 {
        let fixture = completed_fixture();
        let inputs = Inputs::new(&fixture);
        let before = fixture.scan().unwrap().digest;
        let output = fixture.root.join("retirement");
        let displaced = fixture.root.join("displaced");
        let foreign = fixture.root.join("foreign");
        std::fs::create_dir(&foreign).unwrap();
        std::fs::write(foreign.join("sentinel"), b"preserved").unwrap();
        let result = retirement_stage::create_checked(
            fixture.config.clone(),
            inputs.input(),
            &output,
            |boundary| {
                match (case, boundary) {
                    (0, Boundary::Copy) | (1, Boundary::Closed) | (2, Boundary::Ready) => {
                        return Err(invalid())
                    }
                    (3, Boundary::Copy) => {
                        std::fs::rename(&output, &displaced).unwrap();
                        std::os::unix::fs::symlink(&foreign, &output).unwrap();
                    }
                    _ => {}
                }
                Ok(())
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!output.join("ready.json").exists());
        assert!(!displaced.join("ready.json").exists());
        assert_eq!(
            std::fs::read(foreign.join("sentinel")).unwrap(),
            b"preserved"
        );
        assert_eq!(std::fs::read_dir(foreign).unwrap().count(), 1);
        assert_eq!(fixture.scan().unwrap().digest, before);
    }
}

#[tokio::test]
async fn missing_authoritative_deletion_and_uncertain_journals_stay_out_of_archive() {
    for case in 0..3 {
        let fixture = completed_fixture();
        if case == 1 {
            journal(
                &fixture.config,
                &Record::started(&fixture.config, &remove()).unwrap(),
            );
        }
        if case == 2 {
            fixture.write("mutation-authority/.authority-1.tmp", b"uncertain");
        }
        let mut inputs = Inputs::new(&fixture);
        if case == 0 {
            let mut observation = snapshot(&fixture);
            observation.removed_worktrees.clear();
            (inputs.observation, inputs.observation_sha256) =
                write_snapshot(&fixture, &observation);
        }
        let output = fixture.root.join("retirement");
        let before = fixture.scan().unwrap().digest;
        let report = retirement_stage::create(fixture.config.clone(), inputs.input(), &output)
            .await
            .unwrap();
        assert_eq!(report["candidate_resource_groups"], 0);
        assert_eq!(report["archived_records"], 0);
        assert_eq!(fixture.scan().unwrap().digest, before);
    }
}

#[tokio::test]
async fn real_input_archive_crosses_both_legacy_limits_without_deactivating_sources() {
    use layer3_adapter::runtime_ledger_retirement::ledger_retired_worktree_key;
    let fixture = fixture();
    let mut observation = snapshot(&fixture);
    observation.removed_worktrees.clear();
    let groups = crate::mutation_journal::MAX_JOURNAL_ENTRIES + 1;
    observation.agents_observed = groups as u64;
    observation.worktrees_observed = groups as u64;
    for index in 0..groups {
        let name = format!("agent-{index}");
        let operation = Operation::remove_worktree(&layer3_adapter::WorktreeRef::new(
            format!("worktree:workspace:{name}:remove:v1"),
            "workspace",
            &name,
        ));
        for fence in 1..=4 {
            authority(&fixture.config, fence, &operation);
        }
        let completed = Record::started(&fixture.config, &operation)
            .unwrap()
            .completed(&layer3_adapter::WorktreeRemoveReceipt {
                operation_id: operation.operation_id.clone(),
                request_sha256: operation.request_sha256.clone(),
                workspace: "workspace".into(),
                name: name.clone(),
            })
            .unwrap();
        journal(&fixture.config, &completed);
        observation
            .removed_worktrees
            .push(ledger_retired_worktree_key("workspace", &name));
    }
    observation.removed_worktrees.sort();
    let mut inputs = Inputs::new(&fixture);
    (inputs.observation, inputs.observation_sha256) = write_snapshot(&fixture, &observation);
    let before = fixture.scan().unwrap().digest;
    let output = fixture.root.join("retirement");
    let report = retirement_stage::create(fixture.config.clone(), inputs.input(), &output)
        .await
        .unwrap();
    assert_eq!(report["candidate_resource_groups"], groups);
    assert_eq!(report["archived_records"], groups * 5);
    assert_eq!(report["candidate_current_journal_files"], groups);
    assert_eq!(report["candidate_current_authority_files"], groups * 4);
    assert_eq!(fixture.scan().unwrap().digest, before);
    let db = database(&output);
    let pending: i64 = db
        .query_row(
            "SELECT count(*) FROM retired_groups WHERE complete=0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, groups as i64);
}
