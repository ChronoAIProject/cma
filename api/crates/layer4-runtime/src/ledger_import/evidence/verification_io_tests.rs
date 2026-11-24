use super::*;
use std::os::unix::fs::symlink;

#[test]
fn every_artifact_member_is_required_and_additional_entries_refuse() {
    for name in ["evidence.json", "sealed.json"] {
        let artifact = Artifact::new();
        std::fs::remove_file(artifact.path.join(name)).unwrap();
        assert!(artifact.verify().is_err());
        assert!(!artifact.path.join(name).exists());
    }
    for name in [".publish-incomplete.tmp", "unknown", "database"] {
        let artifact = Artifact::new();
        std::fs::write(artifact.path.join(name), b"retained partial artifact").unwrap();
        assert!(artifact.verify().is_err());
        assert_eq!(
            std::fs::read(artifact.path.join(name)).unwrap(),
            b"retained partial artifact"
        );
    }
}

#[test]
fn source_directory_and_file_substitutions_refuse_without_following_them() {
    let artifact = Artifact::new();
    let alias = artifact.fixture.root.join("alias");
    symlink(&artifact.path, &alias).unwrap();
    assert!(verification::verify(
        &alias,
        &artifact.hash,
        &mut Budget::new(limits()),
        || Ok(())
    )
    .is_err());
    for name in ["evidence.json", "sealed.json"] {
        for special in ["symlink", "hardlink", "fifo", "directory"] {
            let artifact = Artifact::new();
            let path = artifact.path.join(name);
            let saved = artifact.fixture.root.join("saved-file");
            std::fs::rename(&path, &saved).unwrap();
            let before = std::fs::read(&saved).unwrap();
            match special {
                "symlink" => symlink(&saved, &path).unwrap(),
                "hardlink" => std::fs::hard_link(&saved, &path).unwrap(),
                "fifo" => {
                    let c = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o400) }, 0);
                }
                "directory" => std::fs::create_dir(&path).unwrap(),
                _ => unreachable!(),
            }
            assert!(artifact.verify().is_err(), "{name}: {special}");
            assert_eq!(std::fs::read(&saved).unwrap(), before);
        }
    }
}

#[test]
fn private_sealed_file_modes_are_required() {
    for name in ["evidence.json", "sealed.json"] {
        for mode in [0o600, 0o440, 0o404, 0o000] {
            let artifact = Artifact::new();
            let path = artifact.path.join(name);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(artifact.verify().is_err(), "{name}: {mode:o}");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            artifact.verify().unwrap();
        }
    }
    let artifact = Artifact::new();
    std::fs::set_permissions(&artifact.path, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(artifact.verify().is_err());
}

#[test]
fn oversized_sparse_files_are_refused_before_parsing_or_allocating_their_contents() {
    for (name, maximum) in [("sealed.json", 4096), ("evidence.json", 16 * 1024 * 1024)] {
        let artifact = Artifact::new();
        let path = artifact.path.join(name);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(maximum + 1).unwrap();
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(artifact.verify().is_err());
        assert_eq!(std::fs::metadata(path).unwrap().len(), maximum + 1);
    }
}

#[test]
fn final_recheck_refuses_payload_seal_and_directory_replacement_even_with_identical_bytes() {
    for selected in ["evidence.json", "sealed.json", "directory"] {
        let artifact = Artifact::new();
        let snapshot = artifact.snapshot();
        let result = verification::verify(
            &artifact.path,
            &artifact.hash,
            &mut Budget::new(limits()),
            || {
                if selected == "directory" {
                    let old = artifact.fixture.root.join("old-artifact");
                    std::fs::rename(&artifact.path, &old).unwrap();
                    std::fs::create_dir(&artifact.path).unwrap();
                    std::fs::set_permissions(
                        &artifact.path,
                        std::fs::Permissions::from_mode(0o700),
                    )
                    .unwrap();
                    for name in ["evidence.json", "sealed.json"] {
                        std::fs::copy(old.join(name), artifact.path.join(name)).unwrap();
                    }
                } else {
                    let path = artifact.path.join(selected);
                    let bytes = std::fs::read(&path).unwrap();
                    std::fs::rename(&path, artifact.fixture.root.join("old-file")).unwrap();
                    std::fs::write(&path, bytes).unwrap();
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400))
                        .unwrap();
                }
                Ok(())
            },
        );
        assert!(result.is_err(), "{selected}");
        for (name, bytes, _, _) in snapshot {
            assert_eq!(std::fs::read(artifact.path.join(name)).unwrap(), bytes);
        }
    }
}

#[test]
fn final_recheck_refuses_same_inode_content_change_and_new_entries() {
    for selected in ["evidence.json", "sealed.json", "extra"] {
        let artifact = Artifact::new();
        let result = verification::verify(
            &artifact.path,
            &artifact.hash,
            &mut Budget::new(limits()),
            || {
                let path = artifact.path.join(selected);
                if selected == "extra" {
                    std::fs::write(path, b"new incomplete member").unwrap();
                } else {
                    write(&path, b"corrupt");
                }
                Ok(())
            },
        );
        assert!(result.is_err(), "{selected}");
    }
}

#[test]
fn initial_and_final_reads_share_the_exact_real_byte_allowance() {
    let artifact = Artifact::with_fixture(mutation_fixture());
    let length = ["evidence.json", "sealed.json"]
        .into_iter()
        .map(|name| std::fs::metadata(artifact.path.join(name)).unwrap().len())
        .sum::<u64>();
    for (allowance, expected) in [(2 * length, true), (2 * length - 1, false)] {
        let mut limit = limits();
        limit.bytes = allowance;
        let mut budget = Budget::new(limit);
        let before = artifact.snapshot();
        let result = verification::verify(&artifact.path, &artifact.hash, &mut budget, || Ok(()));
        assert_eq!(result.is_ok(), expected, "{allowance}");
        if !expected {
            assert!(result.err().unwrap().code.contains("capacity"));
        }
        assert_eq!(artifact.snapshot(), before);
    }
}

#[test]
fn expired_deadline_and_enumeration_limits_do_not_produce_success() {
    let artifact = Artifact::new();
    let mut budget = Budget::new(limits());
    budget.start -= Duration::from_secs(31);
    assert!(verification::verify(&artifact.path, &artifact.hash, &mut budget, || Ok(())).is_err());
    for entries in [0, 3, 4] {
        let mut limit = limits();
        limit.entries = entries;
        assert_eq!(
            verification::verify(
                &artifact.path,
                &artifact.hash,
                &mut Budget::new(limit),
                || Ok(())
            )
            .is_ok(),
            entries == 4
        );
    }
}
