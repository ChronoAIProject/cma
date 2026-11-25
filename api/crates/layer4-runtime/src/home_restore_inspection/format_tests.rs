use super::*;

#[test]
fn supported_formats_preserve_all_bytes_and_always_hold() {
    for (v2, retirement) in [
        (false, 0),
        (false, 1),
        (false, 2),
        (true, 0),
        (true, 1),
        (true, 2),
        (true, 3),
    ] {
        let fixture = Fixture::new();
        if v2 {
            fixture.v2();
        }
        if retirement != 0 {
            fixture.retirement(retirement);
        }
        let before = fixture.snapshot();
        let report = fixture.report();
        held(&report);
        for component in ["binding", "installation", "mutation"] {
            assert_eq!(report[component]["status"], "RECOGNIZED", "{report}");
        }
        assert_eq!(
            report["retirement"]["status"],
            if retirement == 0 {
                "ABSENT"
            } else {
                "RECOGNIZED"
            },
            "{report}"
        );
        assert_eq!(before, fixture.snapshot());
        let encoded = serde_json::to_string(&report).unwrap();
        for private in [
            "user-content",
            "sandbox-inspection",
            "/home/cma",
            "inspection-install",
            "database contents unassessed",
        ] {
            assert!(!encoded.contains(private));
        }
    }
}

#[test]
fn absent_binding_and_pending_install_are_findings_without_recovery() {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.roots[io::STATE].join("home-binding/binding.json")).unwrap();
    let before = fixture.snapshot();
    let report = fixture.report();
    assert_eq!(report["binding"]["reason"], "binding_missing");
    assert_eq!(
        report["installation"]["reason"],
        "installation_binding_missing_or_invalid"
    );
    assert_eq!(before, fixture.snapshot());
    let fixture = Fixture::new();
    fixture.install(true);
    let operation = crate::mutation_journal::Operation::remove_agent_home(
        &layer3_adapter::AgentHomeRemoveRequest::new(
            "agent:pending:home:remove:v1",
            layer3_adapter::AgentSlot::new("pending"),
        ),
    );
    let started = crate::mutation_journal::Record::started(&fixture.config, &operation).unwrap();
    let name = started.inventory_name(&fixture.config);
    fixture.write(
        io::STATE,
        &format!(
            "runtime-mutations/{}",
            name.file_name().unwrap().to_str().unwrap()
        ),
        &started.encode().unwrap(),
    );
    let before = fixture.snapshot();
    let report = fixture.report();
    held(&report);
    assert_eq!(report["installation"]["reason"], "installation_started");
    assert_eq!(report["mutation"]["status"], "RECOGNIZED");
    assert_eq!(before, fixture.snapshot());
}

#[test]
fn identity_root_version_and_strict_shape_mismatches_never_rebind() {
    for field in [
        "home",
        "sandbox",
        "born",
        "state_root",
        "home_root",
        "transfer_root",
        "unknown",
        "version",
        "admission",
    ] {
        let fixture = Fixture::new();
        fixture.v2();
        fixture.retirement(3);
        fixture.edit(io::STATE, "mutation-ledger-v2.current.json", |value| {
            value[field] = match field {
                "born" | "version" => json!(9),
                _ => json!("incompatible"),
            };
        });
        let before = fixture.snapshot();
        let report = fixture.report();
        assert_ne!(report["mutation"]["status"], "RECOGNIZED", "{field}");
        assert_eq!(
            report["retirement"]["reason"],
            "retirement_descriptor_invalid"
        );
        held(&report);
        assert_eq!(before, fixture.snapshot());
    }
    for field in [
        "sandbox_id",
        "home_identity",
        "runtime_generation",
        "unknown",
    ] {
        let fixture = Fixture::new();
        fixture.edit(io::STATE, "home-binding/binding.json", |value| {
            value[field] = if field == "runtime_generation" {
                json!(2)
            } else {
                json!("changed")
            };
        });
        assert_eq!(fixture.report()["binding"]["reason"], "binding_invalid");
    }
}

#[test]
fn each_missing_dependency_is_independent_and_never_repaired() {
    for missing in [
        "mutation-ledger-v2.current.json",
        "mutation-ledger-v2/ledger.sqlite3",
        "mutation-retirement-v1.current.json",
        "mutation-retirement-v1/ledger.sqlite3",
    ] {
        let fixture = Fixture::new();
        fixture.v2();
        fixture.retirement(3);
        std::fs::remove_file(fixture.roots[io::STATE].join(missing)).unwrap();
        let before = fixture.snapshot();
        let report = fixture.report();
        held(&report);
        let broken = if missing.starts_with("mutation-ledger") {
            "mutation"
        } else {
            "retirement"
        };
        assert_eq!(
            report[broken]["status"], "INCONSISTENT",
            "{missing}: {report}"
        );
        if broken == "retirement" {
            assert_eq!(report["mutation"]["status"], "RECOGNIZED");
        }
        assert_eq!(before, fixture.snapshot());
    }
}

#[test]
fn orphan_barriers_and_partial_publications_are_preserved() {
    let fixture = Fixture::new();
    fixture.write(
        io::TRANSFER,
        &format!("runtime-state/{}", crate::ledger_format::BARRIER),
        b"partial",
    );
    assert_eq!(
        fixture.report()["mutation"]["reason"],
        "mutation_manifest_missing"
    );
    for artifact in [
        ".mutation-ledger-v2.fresh.json",
        ".mutation-ledger-v2.preparing",
        ".mutation-retirement-v1.current.tmp",
        ".mutation-retirement-v1.preparing",
    ] {
        let fixture = Fixture::new();
        fixture.v2();
        fixture.retirement(3);
        if artifact.ends_with(".preparing") {
            std::fs::create_dir(fixture.roots[io::STATE].join(artifact)).unwrap();
        } else {
            fixture.write(io::STATE, artifact, b"interrupted publication");
        }
        let before = fixture.snapshot();
        let report = fixture.report();
        let component = if artifact.starts_with(".mutation-ledger") {
            "mutation"
        } else {
            "retirement"
        };
        assert_eq!(report[component]["status"], "INCONSISTENT");
        assert_eq!(before, fixture.snapshot());
    }
}

#[test]
fn journal_sidecars_do_not_trigger_sqlite_recovery() {
    for store in ["mutation-ledger-v2", "mutation-retirement-v1"] {
        for suffix in ["journal", "wal", "shm"] {
            let fixture = Fixture::new();
            fixture.v2();
            fixture.retirement(3);
            fixture.write(
                io::STATE,
                &format!("{store}/ledger.sqlite3-{suffix}"),
                b"hot journal evidence",
            );
            let before = fixture.snapshot();
            let report = fixture.report();
            let component = if store == "mutation-ledger-v2" {
                "mutation"
            } else {
                "retirement"
            };
            assert_eq!(report[component]["reason"], "store_recovery_unassessed");
            held(&report);
            assert_eq!(before, fixture.snapshot());
        }
    }
}

#[test]
fn pending_install_and_binding_publication_leave_temporary_evidence() {
    let fixture = Fixture::new();
    let bytes =
        std::fs::read(fixture.roots[io::TRANSFER].join("runtime-state/install-v2.json")).unwrap();
    fixture.write(io::TRANSFER, "runtime-state/.install-pending.tmp", &bytes);
    let binding =
        std::fs::read(fixture.roots[io::STATE].join("home-binding/binding.json")).unwrap();
    fixture.write(
        io::STATE,
        &format!("home-binding/.home-{}.tmp", "a".repeat(32)),
        &binding,
    );
    let before = fixture.snapshot();
    let report = fixture.report();
    assert_eq!(report["binding"]["reason"], "binding_publication_pending");
    assert_eq!(
        report["installation"]["reason"],
        "installation_publication_pending"
    );
    assert_eq!(before, fixture.snapshot());
}

#[test]
fn unknown_workspace_records_also_hold_v2() {
    let fixture = Fixture::new();
    fixture.v2();
    fixture.write(io::STATE, "workspace-operations/unknown", b"unsupported");
    assert_eq!(fixture.report()["mutation"]["status"], "UNSUPPORTED");
}

#[tokio::test]
async fn real_unfinished_retirement_stays_unassessed_and_byte_identical() {
    use crate::mutation_journal::{Operation, Record};
    use crate::retirement_archive::{group_key, Archive, GroupProof, SourceRecord};
    let fixture = Fixture::new();
    fixture.retirement(1);
    let path = fixture.roots[io::STATE].join("mutation-retirement-v1");
    std::fs::remove_dir_all(&path).unwrap();
    let archive = Archive::open(
        path,
        fixture.config.clone(),
        "22222222-2222-4222-8222-222222222222".into(),
        true,
        262144,
    )
    .await
    .unwrap();
    let operation = Operation::remove_agent_home(&layer3_adapter::AgentHomeRemoveRequest::new(
        "agent:retired:home:remove:v1",
        layer3_adapter::AgentSlot::new("retired"),
    ));
    let journal = Record::started(&fixture.config, &operation)
        .unwrap()
        .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            agent_id: layer3_adapter::AgentSlot::new("retired"),
        })
        .unwrap();
    let authority = crate::mutation_authority::Record::new(&fixture.config, 4, &operation);
    let filename = |path: PathBuf| path.file_name().unwrap().to_str().unwrap().to_owned();
    let records = [
        SourceRecord {
            scope: 0,
            kind: 1,
            name: filename(journal.inventory_name(&fixture.config)),
            bytes: journal.encode().unwrap(),
        },
        SourceRecord {
            scope: 0,
            kind: 2,
            name: filename(authority.inventory_name(&fixture.config)),
            bytes: serde_json::to_vec(&authority).unwrap(),
        },
    ];
    let mut digest = Sha256::new();
    for record in &records {
        record.hash_into(&mut digest);
    }
    let proof = GroupProof {
        key: group_key(&operation),
        source_sha256: crate::command::hex(&digest.finalize()),
        observation_sha256: "b".repeat(64),
        records: 2,
    };
    archive
        .prepare(proof, move |index| Ok(records[index as usize].clone()))
        .await
        .unwrap();
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
    let before = fixture.snapshot();
    let report = fixture.report();
    assert_eq!(
        report["retirement"]["reason"],
        "retirement_metadata_transition_unassessed"
    );
    held(&report);
    assert_eq!(before, fixture.snapshot());
}

#[test]
fn retirement_descriptor_refuses_wrong_authority_even_with_matching_marker() {
    for field in [
        "version",
        "unknown",
        "home",
        "sandbox",
        "state_root",
        "home_root",
        "transfer_root",
        "born",
        "active_v2_sha256",
    ] {
        let fixture = Fixture::new();
        fixture.v2();
        fixture.retirement(3);
        fixture.edit(io::STATE, "mutation-retirement-v1.current.json", |value| {
            value[field] = if field == "version" || field == "born" {
                json!(9)
            } else {
                json!("changed")
            };
        });
        let bytes =
            std::fs::read(fixture.roots[io::STATE].join("mutation-retirement-v1.current.json"))
                .unwrap();
        fixture.write(
            io::TRANSFER,
            &format!("runtime-state/{}", crate::retirement_format::BARRIER),
            &bytes,
        );
        let before = fixture.snapshot();
        let report = fixture.report();
        assert_eq!(report["mutation"]["status"], "RECOGNIZED");
        assert_eq!(
            report["retirement"]["reason"],
            if field == "version" {
                "retirement_version"
            } else {
                "retirement_descriptor_invalid"
            },
            "{field}"
        );
        held(&report);
        assert_eq!(before, fixture.snapshot());
    }
    let fixture = Fixture::new();
    fixture.retirement(3);
    assert_eq!(
        fixture.report()["retirement"]["reason"],
        "retirement_descriptor_invalid"
    );
}
