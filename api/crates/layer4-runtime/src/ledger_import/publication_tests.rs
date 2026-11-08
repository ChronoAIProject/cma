use super::*;
use std::io::Write;

#[test]
fn descriptor_publication_withdraws_ready_and_seal_at_each_failed_barrier() {
    let root = std::env::temp_dir().join(format!("ledger-publication-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let directory = File::open(&root).unwrap();
    for name in ["ready.json", "sealed.json"] {
        for failure in [
            Step::FileSync,
            Step::Rename,
            Step::FinalSync,
            Step::DirectorySync,
            Step::Verify,
        ] {
            let result = write(
                &directory,
                name,
                |file| {
                    file.write_all(b"{\"complete\":true}")
                        .map_err(|_| storage())
                },
                || Ok(()),
                |step| {
                    if step == failure {
                        Err(storage())
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(result.unwrap_err().code, "mutation_ledger_storage_hold");
            assert!(!root.join(name).exists(), "{name}: {failure:?}");
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        }
        write(
            &directory,
            name,
            |file| {
                file.write_all(b"{\"complete\":true}")
                    .map_err(|_| storage())
            },
            || Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        let before = std::fs::read(root.join(name)).unwrap();
        assert!(write(&directory, name, |_| Ok(()), || Ok(()), |_| Ok(())).is_err());
        assert_eq!(std::fs::read(root.join(name)).unwrap(), before);
        std::fs::remove_file(root.join(name)).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_descriptor_withdrawal_neutralizes_only_the_owned_file() {
    let root = std::env::temp_dir().join(format!("ledger-publication-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let directory = File::open(&root).unwrap();
    let error = write(
        &directory,
        "ready.json",
        |file| {
            file.write_all(b"{\"complete\":true}")
                .map_err(|_| storage())
        },
        || Err(invalid()),
        |step| {
            if step == Step::CleanupUnlink {
                Err(storage())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "mutation_ledger_migration_invalid");
    assert!(std::fs::read(root.join("ready.json")).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
