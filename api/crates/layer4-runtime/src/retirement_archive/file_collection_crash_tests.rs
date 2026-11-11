use super::*;
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;

#[tokio::test]
async fn file_completion_process_exit_recovers_partial_moves_and_cold_acknowledgement() {
    for stop in ["partial", "source", "ack"] {
        let files = Files::new();
        files.add("crash-source", true);
        let archive = files.open(true).await;
        let (binding, reference, proof) = files.select(&archive, "crash-source").await;
        archive.close().await.unwrap();
        let bytes = serde_json::to_vec(&(
            &binding,
            &reference,
            &proof.source_sha256,
            &proof.observation_sha256,
            proof.records,
        ))
        .unwrap();
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(files.fixture.root.join("file-completion.json"))
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        let child=std::process::Command::new(std::env::current_exe().unwrap()).args([
            "--exact","retirement_archive::tests::file_completion_tests::crashes::file_completion_crash_helper","--ignored"
        ]).env("CMA_TEST_FILE_COLLECTION_ROOT",&files.fixture.root).env("CMA_TEST_FILE_COLLECTION_STOP",stop).output().unwrap();
        assert_eq!(
            child.status.code(),
            Some(79),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let archive = files.open(false).await;
        assert_eq!(archive.require_ready().await.is_err(), stop != "ack");
        let receipt = complete_file_collection_held(
            archive,
            binding.clone(),
            reference.clone(),
            proof.clone(),
            (),
        )
        .await
        .unwrap();
        assert_eq!(files.counts(), [0, 0]);
        let archive = files.open(false).await;
        assert_eq!(
            complete_file_collection_held(archive, binding, reference, proof, ())
                .await
                .unwrap(),
            receipt
        );
    }
}

#[test]
#[ignore = "invoked by the parent process-exit test"]
fn file_completion_crash_helper() {
    let root = PathBuf::from(std::env::var("CMA_TEST_FILE_COLLECTION_ROOT").unwrap());
    let stop = std::env::var("CMA_TEST_FILE_COLLECTION_STOP").unwrap();
    let (binding, reference, source_sha256, observation_sha256, records): (
        LedgerBindingV1,
        CollectionReference,
        String,
        String,
        u32,
    ) = serde_json::from_slice(&std::fs::read(root.join("file-completion.json")).unwrap()).unwrap();
    let mut config = RuntimeConfig::for_test(root.join("runtime"));
    config.home_identity = Some("a".repeat(43));
    config.home = root.join("files/home");
    config.state_root = root.join("files/state");
    config.checkpoint_root = root.join("files/transfer");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let archive = Archive::open_version(
                root.join("archive"),
                config,
                binding.archive_id.clone(),
                false,
                32768,
                Version::FileCollection,
            )
            .await
            .unwrap();
            let proof = GroupProof {
                key: reference.group_key.clone(),
                source_sha256,
                observation_sha256,
                records,
            };
            complete_checked(archive, binding, reference, proof, (), move |at| {
                if (stop == "partial" && matches!(at, Boundary::FileMoved))
                    || (stop == "source" && matches!(at, Boundary::FilesVerified))
                    || (stop == "ack" && matches!(at, Boundary::Acknowledged))
                {
                    std::process::exit(79);
                }
                Ok(())
            })
            .await
            .unwrap();
        });
    panic!("child missed the requested durable boundary");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_completion_caller_cancellation_keeps_the_owned_exclusion_until_settlement() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let files = Files::new();
    files.add("cancel", true);
    let archive = files.open(true).await;
    let (binding, reference, proof) = files.select(&archive, "cancel").await;
    let dropped = Arc::new(AtomicBool::new(false));
    let (entered, seen) = tokio::sync::oneshot::channel();
    let (release, resume) = std::sync::mpsc::channel();
    let mut entered = Some(entered);
    let task = tokio::spawn(complete_checked(
        archive,
        binding.clone(),
        reference.clone(),
        proof.clone(),
        Guard(dropped.clone()),
        move |at| {
            if matches!(at, Boundary::BeforeFiles) {
                entered.take().unwrap().send(()).unwrap();
                resume
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            }
            Ok(())
        },
    ));
    seen.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!dropped.load(Ordering::SeqCst));
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let archive = files.open(false).await;
    archive.require_ready().await.unwrap();
    assert!(archive
        .collection_completion(binding, reference)
        .await
        .unwrap()
        .is_some());
    archive.close().await.unwrap();
}
