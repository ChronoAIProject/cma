use super::*;

const CHILD_ROOT: &str = "CMA_TEST_DEACTIVATION_ROOT";
const CHILD_ID: &str = "CMA_TEST_DEACTIVATION_ARCHIVE";

#[tokio::test]
async fn indexed_deactivation_process_exit_after_commit_reopens_for_exact_retry() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, true, false).await;
    let input = serde_json::to_vec(&(
        &proof.key,
        &proof.source_sha256,
        &proof.observation_sha256,
        proof.records,
        &observation,
    ))
    .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(fixture.root.join("deactivation-input.json"))
        .unwrap();
    file.write_all(&input).unwrap();
    file.sync_all().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "retirement_archive::tests::indexed_tests::deactivation_tests::crash_tests::indexed_deactivation_crash_helper", "--ignored"])
        .env(CHILD_ROOT, &fixture.root).env(CHILD_ID, &fixture.id).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(78),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source = fixture.active(false).await;
    let after = source.snapshot().await.unwrap();
    assert_eq!(
        (after.operations, after.authority, after.processes),
        (5, 4, 1)
    );
    source.close().await.unwrap();
    let receipt = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    assert_eq!(receipt.after_revision, after.revision as u64);
    assert_eq!(fixture.apply(proof, observation).await.unwrap(), receipt);
}

#[test]
#[ignore = "invoked by the parent process-exit test"]
fn indexed_deactivation_crash_helper() {
    let root = PathBuf::from(std::env::var(CHILD_ROOT).expect("child fixture"));
    let id = std::env::var(CHILD_ID).expect("child archive");
    let (key, source_sha256, observation_sha256, records, observation): (
        String,
        String,
        String,
        u32,
        Vec<u8>,
    ) = serde_json::from_slice(&std::fs::read(root.join("deactivation-input.json")).unwrap())
        .unwrap();
    let mut config = RuntimeConfig::for_test(root.join("runtime"));
    config.home_identity = Some("a".repeat(43));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let archive = Archive::open_version(
                root.join("archive"),
                config.clone(),
                id,
                false,
                32_768,
                Version::Indexed,
            )
            .await
            .unwrap();
            let ledger = Ledger::open(root.join("active"), config, false, 32_768)
                .await
                .unwrap();
            deactivate_checked(
                archive,
                ledger,
                GroupProof {
                    key,
                    source_sha256,
                    observation_sha256,
                    records,
                },
                observation,
                (),
                |boundary| {
                    if matches!(boundary, Boundary::Committed) {
                        std::process::exit(78);
                    }
                    Ok(())
                },
            )
            .await
            .unwrap();
        });
    panic!("child did not reach durable source commit");
}
