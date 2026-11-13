use super::*;
use crate::ledger_reclamation::CollectionEvidence;
use layer3_adapter::runtime_ledger_collection::CollectionGroup;
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;

async fn prepared(
    fixture: &Fixture,
    changed_revision: bool,
    foreign_archive: bool,
) -> (GroupProof, LedgerBindingV1, [Vec<u8>; 3]) {
    let (context, rows, mut proof, _) = fixture.indexed_sources().await;
    let archive_id = if foreign_archive {
        uuid::Uuid::new_v4().to_string()
    } else {
        fixture.id.clone()
    };
    let (binding, originals) = crate::retirement_archive::collection_probe::artifacts(
        &fixture.config,
        &archive_id,
        context.revision().unwrap() + u64::from(changed_revision),
        CollectionGroup::Agent {
            agent_id: "indexed".into(),
        },
    )
    .unwrap();
    let evidence = CollectionEvidence::parse(&binding, &proof.key, originals.clone()).unwrap();
    proof.observation_sha256 = evidence.observation_sha256().into();
    let archive = fixture.indexed(true).await;
    archive
        .prepare_indexed(proof.clone(), context, move |index| {
            Ok(rows[index as usize].clone())
        })
        .await
        .unwrap();
    archive.close().await.unwrap();
    (proof, binding, originals)
}

async fn source(fixture: &Fixture) -> Ledger {
    Ledger::open(
        fixture.root.join("active"),
        fixture.config.clone(),
        false,
        32_768,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn bounded_authority_deactivation_keeps_archive_held_and_retries_original_bytes() {
    let fixture = Fixture::new();
    let (proof, binding, originals) = prepared(&fixture, false, false).await;
    let evidence = || CollectionEvidence::parse(&binding, &proof.key, originals.clone()).unwrap();
    let fingerprint = evidence().fingerprint();
    let first = deactivate_collected_held(
        fixture.indexed(false).await,
        source(&fixture).await,
        proof.clone(),
        evidence(),
        (),
    )
    .await
    .unwrap();
    assert_eq!((first.operations, first.authority), (1, 1));
    let ledger = source(&fixture).await;
    let after = ledger.snapshot().await.unwrap();
    assert_eq!(
        (after.operations, after.authority, after.terminal),
        (0, 0, 0)
    );
    ledger.close().await.unwrap();
    let archive = fixture.indexed(false).await;
    assert!(archive.require_ready().await.is_err());
    assert!(
        !archive
            .verify_indexed(proof.clone())
            .await
            .unwrap()
            .complete
    );
    let operation = fixture.records("indexed").0;
    assert!(archive.authorize(&[operation]).await.is_err());
    let retry_evidence = evidence();
    assert_eq!(retry_evidence.fingerprint(), fingerprint);
    let retry = deactivate_collected_held(
        archive,
        source(&fixture).await,
        proof.clone(),
        retry_evidence,
        (),
    )
    .await
    .unwrap();
    assert_eq!(retry, first);
    // The new bounded artifact remains distinct from the legacy full snapshot.
    assert!(deactivate_held(
        fixture.indexed(false).await,
        source(&fixture).await,
        proof,
        originals[2].clone(),
        ()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn changed_source_revision_or_archive_identity_refuses_before_deactivation() {
    for (changed_revision, foreign_archive) in [(true, false), (false, true)] {
        let fixture = Fixture::new();
        let (proof, binding, originals) =
            prepared(&fixture, changed_revision, foreign_archive).await;
        let ledger = source(&fixture).await;
        let before = ledger.snapshot().await.unwrap();
        let evidence = CollectionEvidence::parse(&binding, &proof.key, originals).unwrap();
        assert!(deactivate_collected_held(
            fixture.indexed(false).await,
            ledger,
            proof.clone(),
            evidence,
            (),
        )
        .await
        .is_err());
        let ledger = source(&fixture).await;
        assert_eq!(ledger.snapshot().await.unwrap(), before);
        ledger.close().await.unwrap();
        let archive = fixture.indexed(false).await;
        assert!(!archive.verify_indexed(proof).await.unwrap().complete);
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
    }
}
