use super::*;
use std::os::unix::fs::symlink;

#[test]
fn request_refuses_unknown_fields_versions_and_root_remapping() {
    let valid = json!({"version":1,"sandbox_id":"sandbox-inspection","home_identity":"a".repeat(43),"generation":3,"layout":"native-runtime-v1"});
    for (field, value) in [
        ("version", json!(2)),
        ("layout", json!("other")),
        ("generation", json!(0)),
        ("sandbox_id", json!("../other")),
        ("home_root", json!("/different")),
    ] {
        let mut input = valid.clone();
        input[field] = value;
        let parsed = serde_json::from_value::<Request>(input);
        assert!(
            parsed
                .map(|request| request.config().is_err())
                .unwrap_or(true),
            "{field}"
        );
    }
}

#[test]
fn production_source_rejects_writable_mounts() {
    let fixture = Fixture::new();
    let config = RuntimeConfig {
        home: fixture.roots[0].clone(),
        state_root: fixture.roots[1].clone(),
        checkpoint_root: fixture.roots[2].clone(),
        ..fixture.config.clone()
    };
    assert_eq!(
        io::Source::open(&config, budget()).err().unwrap().code,
        "restored_home_writable_mount"
    );
}

#[test]
fn symlink_directory_leaf_hardlink_and_fifo_refuse_without_blocking() {
    for mode in ["leaf", "directory", "hardlink", "fifo"] {
        let fixture = Fixture::new();
        let binding = fixture.roots[io::STATE].join("home-binding/binding.json");
        let original = fixture.root.join("original");
        std::fs::rename(&binding, &original).unwrap();
        match mode {
            "leaf" => symlink(&original, &binding).unwrap(),
            "directory" => {
                std::fs::remove_dir(binding.parent().unwrap()).unwrap();
                symlink(&fixture.root, binding.parent().unwrap()).unwrap();
            }
            "hardlink" => std::fs::hard_link(&original, &binding).unwrap(),
            _ => {
                use std::os::unix::ffi::OsStrExt;
                let name = std::ffi::CString::new(binding.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
        }
        let result = assess(
            &fixture.config,
            &mut fixture.source(),
            &sha256(b"request"),
            OUTPUT_BYTES,
        );
        assert!(result.is_err(), "{mode}");
        assert_eq!(std::fs::read(&original).unwrap()[0], b'{');
    }
}

#[test]
fn final_recheck_detects_inplace_replacement_new_entries_and_root_substitution() {
    for mode in ["inplace", "replacement", "entry", "root"] {
        let fixture = Fixture::new();
        let mut source = fixture.source();
        source
            .record(io::STATE, "home-binding/binding.json", 4096)
            .unwrap();
        let binding = fixture.roots[io::STATE].join("home-binding/binding.json");
        match mode {
            "inplace" => {
                fixture.edit(io::STATE, "home-binding/binding.json", |value| {
                    value["runtime_generation"] = json!(2)
                });
            }
            "replacement" => {
                let bytes = std::fs::read(&binding).unwrap();
                std::fs::remove_file(&binding).unwrap();
                write(&binding, &bytes);
            }
            "entry" => fixture.write(io::STATE, "home-binding/new-entry", b"new"),
            _ => {
                std::fs::rename(
                    &fixture.roots[io::STATE],
                    fixture.root.join("replaced-state"),
                )
                .unwrap();
                std::fs::create_dir(&fixture.roots[io::STATE]).unwrap();
            }
        }
        assert_eq!(
            source.finish(&sha256(b"request")).unwrap_err().code,
            "restored_home_source_changed",
            "{mode}"
        );
    }
}

#[test]
fn missing_observation_cannot_become_a_silent_present_dependency() {
    let fixture = Fixture::new();
    let mut source = fixture.source();
    assert!(!source
        .present(io::STATE, "mutation-ledger-v2.current.json")
        .unwrap());
    fixture.write(io::STATE, "mutation-ledger-v2.current.json", b"late");
    assert_eq!(
        source.finish(&sha256(b"request")).unwrap_err().code,
        "restored_home_source_changed"
    );
}

#[test]
fn all_budgets_fail_without_a_complete_partial_report() {
    let fixture = Fixture::new();
    let mut source = fixture.source();
    assert_eq!(
        assess(&fixture.config, &mut source, &sha256(b"request"), 1)
            .unwrap_err()
            .code,
        "restored_home_output_budget"
    );
    let mut source = fixture.source();
    source.elapsed = Some(Duration::ZERO);
    source.check().unwrap();
    source.elapsed = Some(Duration::from_secs(31));
    assert_eq!(
        assess(
            &fixture.config,
            &mut source,
            &sha256(b"request"),
            OUTPUT_BYTES
        )
        .unwrap_err()
        .code,
        "restored_home_time_budget"
    );
    for limits in [
        Limits {
            entries: 1,
            ..Limits::default()
        },
        Limits {
            bytes: 1,
            ..Limits::default()
        },
        Limits {
            metadata_bytes: 1,
            ..Limits::default()
        },
    ] {
        let result = io::Source::fixture(fixture.roots.clone(), Budget::new(limits)).and_then(
            |mut source| {
                assess(
                    &fixture.config,
                    &mut source,
                    &sha256(b"request"),
                    OUTPUT_BYTES,
                )
            },
        );
        assert_eq!(result.unwrap_err().code, "restored_home_metadata_budget");
    }
    fixture.write(io::STATE, "home-binding/binding.json", &vec![b' '; 4097]);
    assert_eq!(
        assess(
            &fixture.config,
            &mut fixture.source(),
            &sha256(b"request"),
            OUTPUT_BYTES
        )
        .unwrap_err()
        .code,
        "restored_home_record_budget"
    );
}

#[test]
fn bounded_request_and_json_topology_refuse_before_source_reads() {
    let fixture = Fixture::new();
    let path = fixture.root.join("request.json");
    write(&path, &vec![b' '; REQUEST_BYTES as usize + 1]);
    assert!(execute(&path).is_err());
    write(&path, br#"{"unexpected":"private request"}"#);
    let error = execute(&path).unwrap_err();
    assert_eq!(error.code, "restored_home_request_invalid");
    assert!(!error.to_string().contains("private request"));
    fixture.write(
        io::TRANSFER,
        "runtime-state/install-v2.json",
        &serde_json::to_vec(&vec![0; 4097]).unwrap(),
    );
    assert_eq!(
        assess(
            &fixture.config,
            &mut fixture.source(),
            &sha256(b"request"),
            OUTPUT_BYTES
        )
        .unwrap_err()
        .code,
        "restored_home_metadata_budget"
    );
}

#[test]
fn publication_metadata_does_not_bypass_safe_io() {
    for name in [
        ".mutation-ledger-v2.current.tmp",
        ".mutation-ledger-v2.fresh.json",
        ".mutation-retirement-v1.current.tmp",
        ".mutation-retirement-v1.preparing",
    ] {
        let fixture = Fixture::new();
        fixture.v2();
        fixture.retirement(3);
        symlink(
            &fixture.roots[io::HOME],
            fixture.roots[io::STATE].join(name),
        )
        .unwrap();
        assert!(
            assess(
                &fixture.config,
                &mut fixture.source(),
                &sha256(b"request"),
                OUTPUT_BYTES
            )
            .is_err(),
            "{name}"
        );
    }
    let fixture = Fixture::new();
    fixture.v2();
    fixture.write(io::STATE, ".mutation-ledger-v2.current.tmp", b"initial");
    let mut source = fixture.source();
    super::super::formats::mutation(&fixture.config, &mut source).unwrap();
    fixture.write(io::STATE, ".mutation-ledger-v2.current.tmp", b"changed");
    assert_eq!(
        source.finish(&sha256(b"request")).unwrap_err().code,
        "restored_home_source_changed"
    );
}
