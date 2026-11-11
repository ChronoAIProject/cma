use super::*;
#[path = "collection_tests.rs"]
mod completion_tests;
#[path = "file_collection_tests.rs"]
mod file_completion_tests;
#[path = "indexed_tests.rs"]
mod indexed_tests;
#[path = "transfer_tests.rs"]
mod transfer_tests;
#[path = "v1_lifetime_tests.rs"]
mod v1_lifetime_tests;
use crate::mutation_authority::Record as Authority;
use crate::mutation_journal::Record as Journal;
use layer3_adapter::{AgentHomeRemoveRequest, AgentSlot};

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
    path: PathBuf,
    id: String,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-retirement-archive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut config = RuntimeConfig::for_test(root.join("runtime"));
        config.home_identity = Some("a".repeat(43));
        Self {
            path: root.join("archive"),
            root,
            config,
            id: uuid::Uuid::new_v4().to_string(),
        }
    }
    async fn open(&self, create: bool) -> Archive {
        Archive::open(
            self.path.clone(),
            self.config.clone(),
            self.id.clone(),
            create,
            32_768,
        )
        .await
        .unwrap()
    }
    fn records(&self, name: &str) -> (Operation, Vec<SourceRecord>, GroupProof) {
        let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
            format!("agent:{name}:home:remove:v1"),
            AgentSlot::new(name),
        ));
        let journal = Journal::started(&self.config, &operation)
            .unwrap()
            .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                operation_id: operation.operation_id.clone(),
                request_sha256: operation.request_sha256.clone(),
                agent_id: AgentSlot::new(name),
            })
            .unwrap();
        let authority = Authority::new(&self.config, 4, &operation);
        let records = vec![
            SourceRecord {
                scope: 0,
                kind: 1,
                name: journal
                    .inventory_name(&self.config)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                bytes: journal.encode().unwrap(),
            },
            SourceRecord {
                scope: 0,
                kind: 2,
                name: authority
                    .inventory_name(&self.config)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                bytes: serde_json::to_vec(&authority).unwrap(),
            },
        ];
        let mut digest = Sha256::new();
        for record in &records {
            record.hash_into(&mut digest);
        }
        let proof = GroupProof {
            key: group_key(&operation),
            source_sha256: format!("{:x}", digest.finalize()),
            observation_sha256: "b".repeat(64),
            records: records.len() as u32,
        };
        (operation, records, proof)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn committed_archive_refuses_pending_and_completed_replays_after_restart() {
    let fixture = Fixture::new();
    let (operation, records, proof) = fixture.records("agent");
    let archive = fixture.open(true).await;
    archive
        .authorize(std::slice::from_ref(&operation))
        .await
        .unwrap();
    let original = records.clone();
    let receipt = archive
        .prepare(proof.clone(), move |index| {
            Ok(records[index as usize].clone())
        })
        .await
        .unwrap();
    assert_eq!(receipt.records, 2);
    assert!(!receipt.complete);
    assert_eq!(
        archive
            .authorize(std::slice::from_ref(&operation))
            .await
            .unwrap_err()
            .code,
        "mutation_resource_retired"
    );
    // Retrying an already committed preparation must not reread missing originals.
    let duplicate = archive
        .prepare(proof.clone(), |_| panic!("source already archived"))
        .await
        .unwrap();
    assert_eq!(receipt, duplicate);
    archive.close().await.unwrap();
    let archive = fixture.open(false).await;
    assert!(archive
        .authorize(std::slice::from_ref(&operation))
        .await
        .is_err());
    archive
        .complete(proof.key.clone(), proof.source_sha256.clone())
        .await
        .unwrap();
    archive
        .complete(proof.key.clone(), proof.source_sha256.clone())
        .await
        .unwrap();
    assert!(archive
        .authorize(std::slice::from_ref(&operation))
        .await
        .is_err());
    archive.close().await.unwrap();
    let db = rusqlite::Connection::open(fixture.path.join("ledger.sqlite3")).unwrap();
    let retained: Vec<Vec<u8>> = db
        .prepare("SELECT payload FROM archived_records ORDER BY scope,kind,source_name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        retained,
        original.into_iter().map(|r| r.bytes).collect::<Vec<_>>()
    );
    drop(db);
}

#[tokio::test]
async fn changed_resource_or_operation_identity_cannot_bypass_retirement() {
    let fixture = Fixture::new();
    let (operation, records, proof) = fixture.records("retired");
    let archive = fixture.open(true).await;
    archive
        .prepare(proof, move |index| Ok(records[index as usize].clone()))
        .await
        .unwrap();
    let same_group = Operation::create_agent_home(&layer3_adapter::AgentHomePlan {
        operation_id: "agent:retired:home:create:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("retired"),
        files: Vec::new(),
        symlinks: Vec::new(),
        skills: Vec::new(),
    });
    let same_operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        &operation.operation_id,
        AgentSlot::new("different"),
    ));
    let fresh = fixture.records("live").0;
    // Current wire identities embed the resource. Altering it already fails
    // validation; a different valid operation on the retired group must also
    // refuse. Keep the scoped operation index as part of the retained proof.
    for (rejected, code) in [
        (&same_group, "mutation_resource_retired"),
        (&same_operation, "mutation_retirement_archive_invalid"),
    ] {
        assert_eq!(
            archive
                .authorize(std::slice::from_ref(rejected))
                .await
                .unwrap_err()
                .code,
            code
        );
        assert!(archive
            .authorize(&[fresh.clone(), rejected.clone()])
            .await
            .is_err());
        assert!(archive
            .authorize(&[rejected.clone(), fresh.clone()])
            .await
            .is_err());
    }
    archive.authorize(&[fresh]).await.unwrap();
    archive.close().await.unwrap();
}

#[tokio::test]
async fn failed_copy_or_mismatched_proof_never_publishes_a_partial_retirement() {
    for failure in 0..6 {
        let fixture = Fixture::new();
        let (operation, mut records, mut proof) = fixture.records("agent");
        let archive = fixture.open(true).await;
        match failure {
            1 => proof.source_sha256 = "f".repeat(64),
            2 => proof.key = "e".repeat(64),
            3 => records[1].bytes = b"malformed record".to_vec(),
            4 => records[1].scope = fixture.config.generation + 1,
            5 => records.reverse(),
            _ => {}
        }
        assert!(
            archive
                .prepare(proof, move |index| {
                    if failure == 0 && index == 1 {
                        return Err(storage_error());
                    }
                    Ok(records[index as usize].clone())
                })
                .await
                .is_err(),
            "case {failure}"
        );
        let authorization = archive.authorize(std::slice::from_ref(&operation)).await;
        if failure == 0 {
            // An ambiguous storage failure consumes the reserve and holds
            // admission even when the later inspection finds no committed row.
            assert_eq!(
                authorization.unwrap_err().code,
                "mutation_ledger_storage_hold"
            );
        } else {
            authorization.unwrap();
        }
        archive.close().await.unwrap();
        let db = rusqlite::Connection::open(fixture.path.join("ledger.sqlite3")).unwrap();
        for table in ["retired_groups", "archived_records", "retired_operations"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "case {failure}, table {table}");
        }
        drop(db);
    }
}

#[tokio::test]
async fn changed_binding_missing_archive_and_replaced_path_fail_closed() {
    let fixture = Fixture::new();
    let archive = fixture.open(true).await;
    archive.close().await.unwrap();
    for case in 0..4 {
        let mut config = fixture.config.clone();
        let mut id = fixture.id.clone();
        match case {
            0 => config.sandbox_id = "foreign".into(),
            1 => config.home_identity = Some("b".repeat(43)),
            2 => config.state_root = fixture.root.join("other-state"),
            _ => id = uuid::Uuid::new_v4().to_string(),
        }
        assert!(
            Archive::open(fixture.path.clone(), config, id, false, 32_768)
                .await
                .is_err()
        );
    }
    let archive = fixture.open(false).await;
    let displaced = fixture.root.join("displaced");
    std::fs::rename(&fixture.path, &displaced).unwrap();
    assert!(archive
        .authorize(&[fixture.records("live").0])
        .await
        .is_err());
    std::os::unix::fs::symlink(&displaced, &fixture.path).unwrap();
    assert!(archive
        .authorize(&[fixture.records("live").0])
        .await
        .is_err());
    archive.close().await.unwrap();
}

#[tokio::test]
async fn indexed_retirements_cross_both_legacy_bounds_without_replay_growth() {
    let fixture = Fixture::new();
    let archive = fixture.open(true).await;
    let sources = Arc::new(probe_sources::Sources::create(&fixture.path, &[]).unwrap());
    let maximum = crate::mutation_journal::MAX_JOURNAL_ENTRIES + 1;
    for index in 0..maximum {
        let (operation, mut records, mut proof) = fixture.records(&format!("agent-{index}"));
        // One receipt and four historical authority claims per completed
        // resource cross both V1 limits after 1,025 retired Agent lifecycles.
        for fence in 1..4 {
            let authority = Authority::new(&fixture.config, fence, &operation);
            records.push(SourceRecord {
                scope: 0,
                kind: 2,
                name: authority
                    .inventory_name(&fixture.config)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
                bytes: serde_json::to_vec(&authority).unwrap(),
            });
        }
        records.sort_by(|a, b| (a.scope, a.kind, &a.name).cmp(&(b.scope, b.kind, &b.name)));
        let mut digest = Sha256::new();
        for record in &records {
            record.hash_into(&mut digest);
        }
        proof.source_sha256 = format!("{:x}", digest.finalize());
        proof.records = records.len() as u32;
        sources.add(&records).unwrap();
        assert_eq!(sources.counts().unwrap(), (5, 5 * index));
        archive
            .authorize(std::slice::from_ref(&operation))
            .await
            .unwrap();
        archive
            .prepare(proof.clone(), move |row| Ok(records[row as usize].clone()))
            .await
            .unwrap();
        let selected = sources.clone();
        archive
            .transfer_group(proof, Direction::Deactivate, move |record| {
                selected.transfer(record, false)
            })
            .await
            .unwrap();
        assert_eq!(sources.counts().unwrap(), (0, 5 * (index + 1)));
        if matches!(index + 1, 1024 | 1025) {
            assert_eq!(
                archive.authorize(&[operation]).await.unwrap_err().code,
                "mutation_resource_retired"
            );
        }
    }
    archive.close().await.unwrap();
    let archive = fixture.open(false).await;
    for _ in 0..16 {
        assert!(archive
            .authorize(&[fixture.records("agent-0").0])
            .await
            .is_err());
    }
    archive
        .authorize(&[fixture.records("new-agent").0])
        .await
        .unwrap();
    archive.close().await.unwrap();
    let db = rusqlite::Connection::open(fixture.path.join("ledger.sqlite3")).unwrap();
    let counts: (i64, i64, i64) = db.query_row(
        "SELECT (SELECT count(*) FROM retired_groups), (SELECT count(*) FROM retired_operations), (SELECT count(*) FROM archived_records)",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(counts, (maximum as i64, maximum as i64, 5 * maximum as i64));
    let authority: i64 = db
        .query_row(
            "SELECT count(*) FROM archived_records WHERE kind=2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(authority > crate::mutation_authority::MAX_AUTHORITY_RECORDS as i64);
    drop(db);
}

#[tokio::test]
async fn detached_archive_writes_remain_in_the_owned_directory_after_path_replacement() {
    use std::os::unix::fs::DirBuilderExt;
    let fixture = Fixture::new();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&fixture.path)
        .unwrap();
    let directory = Arc::new(std::fs::File::open(&fixture.path).unwrap());
    let archive = Archive::create_anchored(
        directory,
        fixture.config.clone(),
        fixture.id.clone(),
        32_768,
    )
    .await
    .unwrap();
    let retained = fixture.root.join("retained");
    let foreign = fixture.root.join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("sentinel"), b"unchanged").unwrap();
    std::fs::rename(&fixture.path, &retained).unwrap();
    std::os::unix::fs::symlink(&foreign, &fixture.path).unwrap();
    let (operation, records, proof) = fixture.records("agent");
    archive
        .prepare(proof, move |index| Ok(records[index as usize].clone()))
        .await
        .unwrap();
    assert!(archive.authorize(&[operation]).await.is_err());
    archive.close().await.unwrap();
    assert_eq!(
        std::fs::read(foreign.join("sentinel")).unwrap(),
        b"unchanged"
    );
    assert_eq!(std::fs::read_dir(&foreign).unwrap().count(), 1);
    let db = rusqlite::Connection::open(retained.join("ledger.sqlite3")).unwrap();
    let count: i64 = db
        .query_row("SELECT count(*) FROM archived_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 2);
    drop(db);
}

#[tokio::test]
async fn synthetic_qualification_is_create_only_and_never_creates_runtime_state() {
    let fixture = Fixture::new();
    let target = fixture.root.join("qualification");
    probe::run(&target).await.unwrap();
    assert!(!target.with_extension("state").exists());
    assert!(!target.with_extension("home").exists());
    assert!(!target.with_extension("transfer").exists());
    assert!(probe::run(&target).await.is_err());
    for extension in ["retirement", "retirement-anchored"] {
        let db =
            rusqlite::Connection::open(target.with_extension(extension).join("ledger.sqlite3"))
                .unwrap();
        let count: i64 = db
            .query_row("SELECT count(*) FROM archived_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }
}

#[tokio::test]
async fn corrupt_archived_payload_cannot_qualify_a_retry_or_completion() {
    let fixture = Fixture::new();
    let (operation, records, proof) = fixture.records("agent");
    let archive = fixture.open(true).await;
    archive
        .prepare(proof.clone(), move |index| {
            Ok(records[index as usize].clone())
        })
        .await
        .unwrap();
    archive.close().await.unwrap();
    let db = rusqlite::Connection::open(fixture.path.join("ledger.sqlite3")).unwrap();
    db.execute(
        "UPDATE archived_records SET sha256=? WHERE kind=1",
        ["f".repeat(64)],
    )
    .unwrap();
    db.close().unwrap();
    let archive = fixture.open(false).await;
    assert!(archive
        .prepare(proof.clone(), |_| panic!("retry cannot reread source"))
        .await
        .is_err());
    assert!(archive
        .complete(proof.key, proof.source_sha256)
        .await
        .is_err());
    assert_eq!(
        archive.authorize(&[operation]).await.unwrap_err().code,
        "mutation_resource_retired"
    );
    archive.close().await.unwrap();
}
