//! Measured real SQLite source/archive completion. Synthetic eligibility is
//! intentionally not C execution authority or an online B adapter.
use super::*;
use crate::mutation_ledger::lifecycle_tests::lifecycle;

async fn cycle(fixture: &Fixture, name: &str, steps: usize) {
    let source = fixture.source(false).await;
    for (index, (operation, receipt)) in lifecycle(&fixture.config, name)
        .into_iter()
        .take(steps)
        .enumerate()
    {
        source
            .admit(index as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        let begun = source.begin(operation).await.unwrap();
        source
            .finish(begun.record.completed(&receipt).unwrap())
            .await
            .unwrap();
    }
    source.close().await.unwrap();
}

async fn select(fixture: &Fixture, name: &str, worktree: bool) -> Intent {
    let group = if worktree {
        CollectionGroup::Worktree {
            workspace: "workspace".into(),
            name: name.into(),
        }
    } else {
        CollectionGroup::Agent {
            agent_id: name.into(),
        }
    };
    let key = group.key();
    let source = fixture.source(false).await;
    let mut cursor = None;
    let mut context = None;
    let mut rows = Vec::new();
    loop {
        let page = source.retirement_inventory(cursor).await.unwrap();
        context.get_or_insert(page.context);
        rows.extend(
            page.rows
                .into_iter()
                .filter(|row| row.group().unwrap().as_deref() == Some(&key)),
        );
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    source.close().await.unwrap();
    let context = context.unwrap();
    let (binding, bytes) = crate::retirement_archive::collection_probe::artifacts(
        &fixture.config,
        &fixture.id,
        context.revision().unwrap(),
        group,
    )
    .unwrap();
    let evidence = CollectionEvidence::parse(&binding, &key, bytes.clone()).unwrap();
    let mut digest = Sha256::new();
    context.hash_into(&mut digest);
    for row in &rows {
        row.hash_into(&mut digest);
    }
    Intent {
        binding,
        bytes,
        proof: GroupProof {
            key,
            source_sha256: format!("{:x}", digest.finalize()),
            observation_sha256: evidence.observation_sha256().into(),
            records: rows.len() as u32,
        },
        context,
        rows,
    }
}

async fn fingerprint(fixture: &Fixture) -> String {
    let source = fixture.source(false).await;
    let mut digest = Sha256::new();
    let mut cursor = None;
    loop {
        let page = source.retirement_inventory(cursor).await.unwrap();
        for row in page.rows {
            row.hash_into(&mut digest);
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    source.close().await.unwrap();
    format!("{:x}", digest.finalize())
}

#[tokio::test]
async fn v2_real_completion_reuses_both_bounds_and_keeps_cold_ack_over_1400_lifecycles() {
    let fixture = Fixture::new();
    fixture.source(true).await.close().await.unwrap();
    fixture.collection(true).await.close().await.unwrap();
    // Preserve a live logical process and a separate unresolved admission.
    cycle(&fixture, "protected-live", 3).await;
    let source = fixture.source(false).await;
    source
        .admit(
            1,
            vec![lifecycle(&fixture.config, "protected-unknown")[0].0.clone()],
        )
        .await
        .unwrap();
    let baseline = source.snapshot().await.unwrap();
    source.close().await.unwrap();
    let protected = fingerprint(&fixture).await;
    let mut original = None;
    let mut removed = [0_u64, 0];
    let mut max_bytes = 0;
    for index in 0..1400 {
        let name = format!("cycle-{index}");
        cycle(&fixture, &name, 6).await;
        let source = fixture.source(false).await;
        let before = source.snapshot().await.unwrap();
        assert_eq!(
            (before.operations, before.authority),
            (baseline.operations + 6, baseline.authority + 3)
        );
        max_bytes = max_bytes.max(before.allocated_bytes);
        source.close().await.unwrap();
        for worktree in [false, true] {
            let intent = select(&fixture, &name, worktree).await;
            let archive = fixture.collection(false).await;
            let reference = intent.retain(&archive).await;
            assert!(archive.require_ready().await.is_err());
            archive.close().await.unwrap();
            // Cold discovery and exact original bytes precede source mutation.
            let archive = fixture.collection(false).await;
            assert_eq!(
                archive
                    .pending_collection(intent.binding.clone())
                    .await
                    .unwrap(),
                Some(reference.clone())
            );
            assert_eq!(
                archive
                    .read_collection(intent.binding.clone(), reference.clone())
                    .await
                    .unwrap()
                    .originals(),
                intent.bytes.each_ref().map(Vec::as_slice)
            );
            intent.prepare(&archive).await;
            archive.close().await.unwrap();
            let reply = fixture.finish(&intent, reference.clone()).await;
            let archive = fixture.collection(false).await;
            let receipt = archive
                .collection_completion(intent.binding.clone(), reference.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reply, receipt);
            assert!(archive
                .pending_collection(intent.binding.clone())
                .await
                .unwrap()
                .is_none());
            archive.require_ready().await.unwrap();
            archive.close().await.unwrap();
            removed[0] += receipt.operations;
            removed[1] += receipt.authority;
            if original.is_none() {
                original = Some((intent, reference, receipt));
            }
        }
        let source = fixture.source(false).await;
        let after = source.snapshot().await.unwrap();
        assert_eq!(
            (
                after.operations,
                after.authority,
                after.terminal,
                after.pending,
                after.processes,
                after.reserved_bytes
            ),
            (
                baseline.operations,
                baseline.authority,
                baseline.terminal,
                baseline.pending,
                baseline.processes,
                baseline.reserved_bytes
            )
        );
        source.close().await.unwrap();
        assert_eq!(fingerprint(&fixture).await, protected);
    }
    assert_eq!(removed, [8400, 4200]);
    assert!(max_bytes < 256 * 4096);
    let archive = fixture.collection(false).await;
    let counts = archive
        .storage
        .call(|db| {
            let query = |sql| {
                db.query_row(sql, [], |r| r.get::<_, i64>(0))
                    .map_err(|_| invalid())
            };
            Ok([
                query("SELECT count(*) FROM retired_groups WHERE complete=1")?,
                query("SELECT count(*) FROM collection_acknowledgements")?,
                query("SELECT count(*) FROM collection_pending")?,
            ])
        })
        .await
        .unwrap();
    assert_eq!(counts, [2800, 2800, 0]);
    // Archive refusal precedes any source admission. Higher-fence serving
    // integration is qualified separately by the supervisor contract suite.
    for name in ["cycle-0", "cycle-1399"] {
        for (operation, _) in lifecycle(&fixture.config, name) {
            assert_eq!(
                archive.authorize(&[operation]).await.unwrap_err().code,
                "mutation_resource_retired"
            );
        }
    }
    archive.close().await.unwrap();
    let (intent, reference, expected) = original.unwrap();
    assert_eq!(fixture.finish(&intent, reference).await, expected);
    assert_eq!(fingerprint(&fixture).await, protected);
    println!("V2_STORAGE_PROOF cycles=1400 retired_operations={} retired_authority={} active_operations={} active_authority={} groups={} cold_acknowledgements={} pending_intents={} max_active_bytes={} originals_exact=true cold_retry=true synthetic_exclusion=true",
        removed[0], removed[1], baseline.operations, baseline.authority, counts[0], counts[1], counts[2], max_bytes);
}
