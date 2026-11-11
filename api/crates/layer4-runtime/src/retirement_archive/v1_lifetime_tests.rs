//! Real V1 files and held transfer, with synthetic authority only. This cannot
//! qualify a production writer fence or the B/C execution coordinator.
use super::*;
use crate::ledger_import::retirement_move::Pair;
use crate::mutation_ledger::lifecycle_tests::lifecycle;
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

fn records(fixture: &Fixture, name: &str, complete: bool) -> Vec<SourceRecord> {
    let mut records = Vec::new();
    let mut authority = BTreeMap::new();
    for (index, (operation, receipt)) in lifecycle(&fixture.config, name).into_iter().enumerate() {
        if !complete && index > 1 {
            break;
        }
        let started = Journal::started(&fixture.config, &operation).unwrap();
        let journal = if complete {
            started.completed(&receipt).unwrap()
        } else {
            started
        };
        let record = Authority::new(&fixture.config, index as u64 + 1, &operation);
        let scope =
            crate::mutation_scope::generation(&fixture.config, &operation.subject).unwrap_or(0);
        records.push(SourceRecord {
            scope,
            kind: 1,
            name: journal
                .inventory_name(&fixture.config)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .into(),
            bytes: journal.encode().unwrap(),
        });
        let name = record
            .inventory_name(&fixture.config)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        authority.insert(
            (scope, name.clone()),
            SourceRecord {
                scope,
                kind: 2,
                name,
                bytes: serde_json::to_vec(&record).unwrap(),
            },
        );
    }
    records.extend(authority.into_values());
    records.sort_by(|a, b| (a.scope, a.kind, &a.name).cmp(&(b.scope, b.kind, &b.name)));
    records
}

fn directory(root: &Path, retained: bool, record: &SourceRecord) -> PathBuf {
    root.join(format!(
        "{}-{}-{}",
        if retained { "retained" } else { "active" },
        record.scope,
        record.kind
    ))
}

fn add(root: &Path, records: &[SourceRecord]) {
    for record in records {
        for retained in [false, true] {
            let directory = directory(root, retained, record);
            if !directory.exists() {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(directory)
                    .unwrap();
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory(root, false, record).join(&record.name))
            .unwrap();
        file.write_all(&record.bytes).unwrap();
        file.sync_all().unwrap();
    }
}

fn counts(root: &Path, retained: bool) -> [usize; 2] {
    let mut counts = [0, 0];
    let prefix = if retained { "retained-" } else { "active-" };
    for directory in std::fs::read_dir(root).unwrap() {
        let directory = directory.unwrap();
        let name = directory.file_name().into_string().unwrap();
        if name.starts_with(prefix) {
            let kind = name.rsplit('-').next().unwrap().parse::<usize>().unwrap();
            counts[kind - 1] += std::fs::read_dir(directory.path()).unwrap().count();
        }
    }
    counts
}

#[tokio::test]
async fn v1_real_files_reuse_both_bounds_and_preserve_originals_over_1400_lifecycles() {
    let fixture = Fixture::new();
    let protected = records(&fixture, "unresolved", false);
    add(&fixture.root, &protected);
    fixture.open(true).await.close().await.unwrap();
    let mut original = None;
    for cycle in 0..1400 {
        let name = format!("cycle-{cycle}");
        let rows = records(&fixture, &name, true);
        assert_eq!(
            (
                rows.iter().filter(|r| r.kind == 1).count(),
                rows.iter().filter(|r| r.kind == 2).count()
            ),
            (6, 3)
        );
        add(&fixture.root, &rows);
        assert_eq!(counts(&fixture.root, false), [8, 5]);
        for operation in [
            lifecycle(&fixture.config, &name)[0].0.clone(),
            lifecycle(&fixture.config, &name)[1].0.clone(),
        ] {
            let key = group_key(&operation);
            let selected: Vec<_> = rows
                .iter()
                .filter(|r| group_key(&r.operation(&fixture.config).unwrap()) == key)
                .cloned()
                .collect();
            let mut digest = Sha256::new();
            for row in &selected {
                row.hash_into(&mut digest);
            }
            let proof = GroupProof {
                key,
                source_sha256: format!("{:x}", digest.finalize()),
                observation_sha256: "b".repeat(64),
                records: selected.len() as u32,
            };
            let archive = fixture.open(false).await;
            archive
                .prepare(proof.clone(), move |i| Ok(selected[i as usize].clone()))
                .await
                .unwrap();
            archive.close().await.unwrap();
            let archive = fixture.open(false).await;
            let root = fixture.root.clone();
            let result = archive
                .transfer_group(proof.clone(), Direction::Deactivate, move |record| {
                    Pair::open(
                        &directory(&root, false, record),
                        &directory(&root, true, record),
                    )?
                    .transfer(&record.name, &record.bytes, false, |_| Ok(()))
                })
                .await
                .unwrap();
            assert!(result.complete);
            if cycle == 0 && original.is_none() {
                original = Some((proof, result));
            }
            archive.close().await.unwrap();
        }
        assert_eq!(counts(&fixture.root, false), [2, 2]);
        for row in &rows {
            assert_eq!(
                std::fs::read(directory(&fixture.root, true, row).join(&row.name)).unwrap(),
                row.bytes
            );
        }
        for row in &protected {
            assert_eq!(
                std::fs::read(directory(&fixture.root, false, row).join(&row.name)).unwrap(),
                row.bytes
            );
        }
    }
    assert_eq!(counts(&fixture.root, true), [8400, 4200]);
    let archive = fixture.open(false).await;
    archive.require_ready().await.unwrap();
    let archived = archive
        .storage
        .call(|db| {
            let query = |sql| {
                db.query_row(sql, [], |r| r.get::<_, i64>(0))
                    .map_err(|_| invalid())
            };
            Ok([
                query("SELECT count(*) FROM retired_groups WHERE complete=1")?,
                query("SELECT count(*) FROM archived_records WHERE kind=1")?,
                query("SELECT count(*) FROM archived_records WHERE kind=2")?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(archived, [2800, 8400, 4200]);
    let (proof, receipt) = original.unwrap();
    let root = fixture.root.clone();
    assert_eq!(
        archive
            .transfer_group(proof, Direction::Deactivate, move |record| {
                Pair::open(
                    &directory(&root, false, record),
                    &directory(&root, true, record),
                )?
                .transfer(&record.name, &record.bytes, false, |_| Ok(()))
            })
            .await
            .unwrap(),
        receipt
    );
    for name in ["cycle-0", "cycle-1399"] {
        for (operation, _) in lifecycle(&fixture.config, name) {
            assert_eq!(
                archive.authorize(&[operation]).await.unwrap_err().code,
                "mutation_resource_retired"
            );
        }
    }
    archive.close().await.unwrap();
    println!("V1_STORAGE_PROOF cycles=1400 retired_operations=8400 retired_authority=4200 active_operations=2 active_authority=2 groups=2800 originals_exact=true cold_retry=true replay_refused=true synthetic_exclusion=true");
}
