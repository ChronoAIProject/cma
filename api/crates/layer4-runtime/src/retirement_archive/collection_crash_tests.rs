use super::*;

#[tokio::test]
async fn collection_process_exit_before_and_after_ack_reopens_for_exact_completion() {
    for stop in ["source", "ack"] {
        let fixture = Fixture::new();
        let intent = fixture.intent("selected", true).await;
        let archive = fixture.collection(true).await;
        let reference = intent.retain(&archive).await;
        intent.prepare(&archive).await;
        archive.close().await.unwrap();
        let input = serde_json::to_vec(&(
            &intent.binding,
            &reference,
            &intent.proof.source_sha256,
            &intent.proof.observation_sha256,
            intent.proof.records,
        ))
        .unwrap();
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(fixture.root.join("completion-input.json"))
            .unwrap();
        file.write_all(&input).unwrap();
        file.sync_all().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "retirement_archive::tests::completion_tests::crash::collection_crash_helper",
                "--ignored",
            ])
            .env("CMA_TEST_COLLECTION_ROOT", &fixture.root)
            .env("CMA_TEST_COLLECTION_STOP", stop)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(79),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let archive = fixture.collection(false).await;
        assert_eq!(archive.require_ready().await.is_err(), stop == "source");
        archive.close().await.unwrap();
        let source = fixture.source(false).await;
        let before = source.snapshot().await.unwrap();
        assert_eq!((before.operations, before.authority), (0, 0));
        source.close().await.unwrap();
        let first = fixture.finish(&intent, reference.clone()).await;
        assert_eq!(fixture.finish(&intent, reference).await, first);
        let source = fixture.source(false).await;
        assert_eq!(source.snapshot().await.unwrap(), before);
        source.close().await.unwrap();
    }
}

#[test]
#[ignore = "invoked by the parent process-exit test"]
fn collection_crash_helper() {
    let root = PathBuf::from(std::env::var("CMA_TEST_COLLECTION_ROOT").unwrap());
    let stop = std::env::var("CMA_TEST_COLLECTION_STOP").unwrap();
    let (binding, reference, source_sha256, observation_sha256, records): (
        LedgerBindingV1,
        CollectionReference,
        String,
        String,
        u32,
    ) = serde_json::from_slice(&std::fs::read(root.join("completion-input.json")).unwrap())
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
                binding.archive_id.clone(),
                false,
                32_768,
                Version::Collection,
            )
            .await
            .unwrap();
            let ledger = Ledger::open(root.join("active"), config, false, 32_768)
                .await
                .unwrap();
            let proof = GroupProof {
                key: reference.group_key.clone(),
                source_sha256,
                observation_sha256,
                records,
            };
            complete_checked(archive, ledger, binding, reference, proof, (), move |at| {
                if (stop == "source" && matches!(at, CollectionBoundary::SourceCommitted))
                    || (stop == "ack" && matches!(at, CollectionBoundary::Acknowledged))
                {
                    std::process::exit(79);
                }
                Ok(())
            })
            .await
            .unwrap();
        });
    panic!("child did not reach its commit boundary");
}
