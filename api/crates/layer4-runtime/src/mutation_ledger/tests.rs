use std::path::PathBuf;

use layer3_adapter::{AgentCloseRequest, AgentSlot, WorktreeRef, WorktreeRequest};

use super::*;
use crate::mutation_authority::{self as v1, Record};

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-indexed-ledger-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self {
            config: RuntimeConfig::for_test(root.join("home")),
            root,
        }
    }
    fn database(&self) -> PathBuf {
        self.root.join("ledger")
    }
    async fn open(&self, create: bool) -> Ledger {
        Ledger::open(self.database(), self.config.clone(), create, 4096)
            .await
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn add(identity: u8) -> Operation {
    Operation::add_worktree(&WorktreeRequest::new(
        "worktree:workspace:agent:add:v1",
        "workspace",
        "agent",
        (if identity == 0 { "a" } else { "b" }).repeat(40),
    ))
}
fn remove() -> Operation {
    Operation::remove_worktree(&WorktreeRef::new(
        "worktree:workspace:agent:remove:v1",
        "workspace",
        "agent",
    ))
}
fn close() -> Operation {
    Operation::close_agent(&AgentCloseRequest::new(
        "agent:agent:close:v1",
        AgentSlot::new("agent"),
    ))
}

#[tokio::test]
async fn fences_fold_to_one_row_with_first_absent_and_survive_reopen() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    let config = fixture.config.clone();
    ledger
        .storage
        .call(move |db| {
            let transaction = db.transaction().unwrap();
            for fence in 1..=10_000 {
                authority::admit(&transaction, &config, fence, &add(0))?;
            }
            authority::admit(&transaction, &config, 10_000, &remove())?;
            authority::admit(&transaction, &config, u64::MAX, &remove())?;
            transaction.commit().unwrap();
            let rows = authority::load(db, &config, &v1::resource_operation(&remove()).0, &[0; 8])?;
            assert_eq!(
                rows.iter().map(|r| r.authority_fence).collect::<Vec<_>>(),
                [10_000, 10_000, u64::MAX]
            );
            assert_eq!(
                db.query_row("SELECT count(*) FROM authority", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    assert_eq!(
        ledger
            .admit(u64::MAX - 1, vec![remove()])
            .await
            .unwrap_err()
            .code,
        "mutation_authority_stale"
    );
    assert_eq!(
        ledger.admit(u64::MAX, vec![add(0)]).await.unwrap_err().code,
        "mutation_authority_retired"
    );
    ledger.admit(u64::MAX, vec![remove()]).await.unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn preparation_pair_is_atomic_and_invalid_identities_never_commit() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger.admit(5, vec![add(0)]).await.unwrap();
    assert_eq!(
        ledger
            .admit(6, vec![close(), add(1)])
            .await
            .unwrap_err()
            .code,
        "mutation_authority_conflict"
    );
    ledger.admit(1, vec![close()]).await.unwrap(); // Failed pair did not advance this fence.
    assert!(ledger.admit(0, vec![remove()]).await.is_err());
    assert!(ledger.admit(6, vec![]).await.is_err());
    assert!(ledger
        .admit(6, vec![remove(), remove(), remove()])
        .await
        .is_err());
    let mut invalid = remove();
    invalid.payload_sha256 = "c".repeat(64);
    assert!(ledger.admit(6, vec![invalid]).await.is_err());
    ledger.admit(6, vec![remove(), close()]).await.unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn indexed_authorization_matches_complete_v1_histories_and_independent_model() {
    use std::collections::BTreeSet;
    type Row = (u64, bool, u8);
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    let config = fixture.config.clone();
    ledger
        .storage
        .call(move |db| {
            let operation = |row: Row| {
                let mut operation = if row.1 { remove() } else { add(row.2) };
                // The bounded model also checks a conflicting Absent identity;
                // API shape validation is tested separately before admission.
                if row.1 && row.2 == 1 {
                    operation.payload_sha256 = "c".repeat(64);
                }
                operation
            };
            let incoming = [1, 2, 3, 1 << 63, u64::MAX]
                .into_iter()
                .flat_map(|f| {
                    [false, true]
                        .into_iter()
                        .flat_map(move |d| [0, 1].map(|i| (f, d, i)))
                })
                .collect::<Vec<_>>();
            let mut seen = BTreeSet::from([Vec::<Row>::new()]);
            let mut frontier = seen.clone();
            let mut comparisons = 0;
            while !frontier.is_empty() {
                let mut next = BTreeSet::new();
                for history in frontier {
                    let transaction = db.transaction().unwrap();
                    let records = history
                        .iter()
                        .map(|r| Record::new(&config, r.0, &operation(*r)))
                        .collect::<Vec<_>>();
                    let valid_history = history.iter().all(|row| !row.1 || row.2 == 0);
                    if valid_history {
                        for row in &history {
                            authority::admit(&transaction, &config, row.0, &operation(*row))?;
                        }
                    }
                    for row in &incoming {
                        let incoming_record = Record::new(&config, row.0, &operation(*row));
                        let v1_result =
                            v1::authorize(&records.iter().collect::<Vec<_>>(), &incoming_record)
                                .map(|_| ())
                                .map_err(|e| e.code);
                        // Independent bounded rule from the reviewed model, including
                        // two abstract identities for each disposition.
                        let result = if history.iter().any(|r| r.1) && !row.1 {
                            Err("mutation_authority_retired".to_string())
                        } else if history.iter().any(|r| r.0 > row.0) {
                            Err("mutation_authority_stale".to_string())
                        } else if history.iter().any(|r| r.1 == row.1 && r.2 != row.2) {
                            Err("mutation_authority_conflict".to_string())
                        } else {
                            Ok(())
                        };
                        assert_eq!(result, v1_result, "{history:?} -> {row:?}");
                        if valid_history {
                            transaction.execute_batch("SAVEPOINT comparison").unwrap();
                            let indexed =
                                authority::admit(&transaction, &config, row.0, &operation(*row))
                                    .map_err(|e| e.code);
                            transaction
                                .execute_batch("ROLLBACK TO comparison; RELEASE comparison")
                                .unwrap();
                            assert_eq!(indexed, result, "indexed {history:?} -> {row:?}");
                        }
                        comparisons += 1;
                        if result.is_ok() {
                            let mut new = history.clone();
                            if !new.contains(row) {
                                new.push(*row);
                                new.sort();
                            }
                            if !seen.contains(&new) {
                                next.insert(new);
                            }
                        }
                    }
                }
                seen.extend(next.iter().cloned());
                frontier = next;
            }
            assert_eq!(seen.len(), 641);
            assert_eq!(comparisons, 12_820);
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn schema_identity_types_and_survivor_order_fail_closed() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger.admit(1, vec![add(0)]).await.unwrap();
    ledger.admit(2, vec![remove()]).await.unwrap();
    ledger.admit(3, vec![remove()]).await.unwrap();
    ledger
        .storage
        .call(|db| {
            assert!(db
                .execute("UPDATE authority SET high_water = 1", [])
                .is_err());
            assert!(db
                .execute("UPDATE authority SET high_water = x'01'", [])
                .is_err());
            assert!(db
                .execute("UPDATE authority SET high_water = zeroblob(8)", [])
                .is_err());
            assert!(db
                .execute("UPDATE authority SET last_absent = NULL", [])
                .is_err());
            db.execute(
                "UPDATE authority SET first_absent=last_absent,last_absent=first_absent",
                [],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        ledger.admit(4, vec![remove()]).await.unwrap_err().code,
        "mutation_ledger_invalid"
    );
    ledger.close().await.unwrap();
    let mut wrong = fixture.config.clone();
    wrong.sandbox_id = "different".into();
    assert!(Ledger::open(fixture.database(), wrong, false, 4096)
        .await
        .is_err());
    let ledger = fixture.open(false).await;
    ledger
        .storage
        .call(|db| {
            db.execute("CREATE TABLE unexpected (value INTEGER)", [])
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
    assert!(
        Ledger::open(fixture.database(), fixture.config.clone(), false, 4096)
            .await
            .is_err()
    );
    assert!(fence::decode(&[0; 8]).is_err());
    assert!(fence::decode(&[1; 7]).is_err());
    assert_eq!(fence::decode(&fence::encode(u64::MAX)).unwrap(), u64::MAX);
}

#[tokio::test]
async fn generation_roll_forward_preserves_filesystem_tombstones_and_isolates_processes() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(64));
    let ledger = fixture.open(true).await;
    ledger.admit(40, vec![close(), remove()]).await.unwrap();
    ledger.close().await.unwrap();
    fixture.config.generation = 2;
    let ledger = fixture.open(false).await;
    ledger.admit(1, vec![close()]).await.unwrap();
    assert_eq!(
        ledger.admit(41, vec![add(0)]).await.unwrap_err().code,
        "mutation_authority_retired"
    );
    assert_eq!(
        ledger.admit(39, vec![remove()]).await.unwrap_err().code,
        "mutation_authority_stale"
    );
    ledger.close().await.unwrap();
    fixture.config.generation = 1;
    assert!(
        Ledger::open(fixture.database(), fixture.config.clone(), false, 4096)
            .await
            .is_err()
    );
    fixture.config.generation = 2;
    fixture.config.home_identity = Some("b".repeat(64));
    assert!(
        Ledger::open(fixture.database(), fixture.config.clone(), false, 4096)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn parent_home_tombstone_blocks_skills_without_erasing_other_authority() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    let skill = Operation::install_agent_skill(&layer3_adapter::AgentSkillInstallRequest::new(
        AgentSlot::new("agent"),
        layer3_adapter::AgentSkill {
            id: "research".into(),
            name: None,
            version: Some("1".into()),
            archive_sha256: "a".repeat(64),
            source_url: "/api/v1/skills/research/versions/1/download".into(),
        },
    ));
    ledger.admit(1, vec![skill.clone()]).await.unwrap();
    let remove_home = Operation::remove_agent_home(&layer3_adapter::AgentHomeRemoveRequest::new(
        "agent:agent:home:remove:v1",
        AgentSlot::new("agent"),
    ));
    ledger.admit(2, vec![remove_home]).await.unwrap();
    assert_eq!(
        ledger.admit(3, vec![skill]).await.unwrap_err().code,
        "mutation_authority_retired"
    );
    ledger.admit(3, vec![add(0)]).await.unwrap();
    ledger.close().await.unwrap();
}
