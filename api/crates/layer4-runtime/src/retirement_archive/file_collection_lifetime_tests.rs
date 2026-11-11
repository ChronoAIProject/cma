use super::*;

#[tokio::test]
async fn file_completion_reuses_both_stores_over_1400_lifecycles_with_cold_original_receipts() {
    let files = Files::new();
    let live = files.add("unresolved", false);
    let mut original = None;
    for cycle_number in 0..1400 {
        let name = format!("file-cycle-{cycle_number}");
        let cycle =
            crate::mutation_ledger::lifecycle_tests::lifecycle(&files.fixture.config, &name);
        let originals = write_cycle(&files, &cycle);
        assert_eq!(files.counts(), [7, 7]);
        for (index, group) in [
            CollectionGroup::Worktree {
                workspace: "workspace".into(),
                name: name.clone(),
            },
            CollectionGroup::Agent {
                agent_id: name.clone(),
            },
        ]
        .into_iter()
        .enumerate()
        {
            let archive = files.open(cycle_number == 0 && index == 0).await;
            let (binding, reference, proof) = files.select_group(&archive, group).await;
            let receipt = complete_file_collection_held(
                archive,
                binding.clone(),
                reference.clone(),
                proof.clone(),
                (),
            )
            .await
            .unwrap();
            let expected = if index == 0 { 2 } else { 4 };
            assert_eq!(
                (receipt.operations, receipt.authority),
                (expected, expected)
            );
            if original.is_none() {
                original = Some((binding, reference, proof, receipt));
            }
        }
        assert_eq!(files.counts(), [1, 1]);
        for (scope, kind, name, bytes) in originals {
            let path = files
                .fixture
                .config
                .checkpoint_root
                .join("runtime-ledger-retired-v4")
                .join(&files.fixture.id)
                .join(format!("{scope}-{kind}"))
                .join(name);
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        let archive = files.open(false).await;
        archive.require_ready().await.unwrap();
        for (operation, _) in cycle {
            assert!(archive.authorize(&[operation]).await.is_err());
        }
        archive
            .authorize(std::slice::from_ref(&live))
            .await
            .unwrap();
        archive.close().await.unwrap();
    }
    let (binding, reference, proof, receipt) = original.unwrap();
    let archive = files.open(false).await;
    assert_eq!(
        complete_file_collection_held(archive, binding, reference, proof, ())
            .await
            .unwrap(),
        receipt
    );
    let archive = files.open(false).await;
    let counts = archive
        .storage
        .call(|db| {
            Ok((
                db.query_row(
                    "SELECT count(*) FROM retired_groups WHERE complete=1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(|_| invalid())?,
                db.query_row(
                    "SELECT count(*) FROM collection_acknowledgements",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(|_| invalid())?,
                db.query_row("SELECT count(*) FROM collection_pending", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|_| invalid())?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(counts, (2800, 2800, 0));
    archive.close().await.unwrap();
    println!("V1_COMPLETION_PROOF lifecycles=1400 archived_journals=8400 archived_authority=8400 cold_receipts=2800 pending=0 live_journals=1 live_authority=1 synthetic_exclusion=true");
}

#[tokio::test]
async fn file_completion_reclaims_at_and_over_both_legacy_limits_without_a_serving_scan() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let files = Files::new();
    for i in 0..1025 {
        files.add(&format!("bound-{i}"), true);
    }
    for i in 1025..4097 {
        let name = format!("bound-{i}");
        let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
            format!("agent:{name}:home:remove:v1"),
            AgentSlot::new(&name),
        ));
        let value = Authority::new(&files.fixture.config, 1, &operation);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(value.inventory_name(&files.fixture.config))
            .unwrap();
        file.write_all(&serde_json::to_vec(&value).unwrap())
            .unwrap();
        file.sync_all().unwrap();
    }
    assert_eq!(files.counts(), [1025, 4097]);
    for (index, name) in ["bound-0", "bound-1"].into_iter().enumerate() {
        let archive = files.open(index == 0).await;
        let (binding, reference, proof) = files.select(&archive, name).await;
        complete_file_collection_held(archive, binding, reference, proof, ())
            .await
            .unwrap();
        assert_eq!(files.counts(), [1024 - index, 4096 - index]);
    }
}

fn write_cycle(
    files: &Files,
    cycle: &[(Operation, serde_json::Value)],
) -> Vec<(u64, u8, String, Vec<u8>)> {
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };
    let config = &files.fixture.config;
    let mut originals = Vec::new();
    for (index, (operation, receipt)) in cycle.iter().enumerate() {
        let journal = Journal::started(config, operation)
            .unwrap()
            .completed(receipt)
            .unwrap();
        let authority = Authority::new(config, index as u64 + 1, operation);
        let scope = crate::mutation_scope::generation(config, &operation.subject).unwrap_or(0);
        for (kind, path, bytes) in [
            (1, journal.inventory_name(config), journal.encode().unwrap()),
            (
                2,
                authority.inventory_name(config),
                serde_json::to_vec(&authority).unwrap(),
            ),
        ] {
            let directory = path.parent().unwrap();
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)
                .unwrap();
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
            std::fs::File::open(directory).unwrap().sync_all().unwrap();
            originals.push((
                scope,
                kind,
                path.file_name().unwrap().to_str().unwrap().into(),
                bytes,
            ));
        }
    }
    originals
}
