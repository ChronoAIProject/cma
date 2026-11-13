use super::*;
use std::os::unix::fs::{symlink, MetadataExt};
fn snapshot(roots: &[&Path]) -> Vec<(io::Identity, Vec<String>)> {
    roots
        .iter()
        .map(|path| {
            let directory = io::Directory::open(path).unwrap();
            (
                directory.identity().unwrap(),
                directory
                    .names(&mut Budget::new(Limits::default()))
                    .unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn physical_and_lexical_output_aliases_refuse_before_creating_any_destination() {
    let fixture = plans::fixture();
    let plan_path = fixture.root.join("plan");
    let planned = plan::create(&fixture.config, &plan_path).unwrap();
    let digest = planned["plan_sha256"].as_str().unwrap();
    let roots = [
        fixture.config.home.as_path(),
        fixture.config.state_root.as_path(),
        fixture.config.checkpoint_root.as_path(),
        &plan_path,
    ];
    let before = snapshot(&roots);
    let ancestor_alias = fixture.root.with_file_name(
        fixture
            .root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_uppercase(),
    );
    let mut physical_aliases = 0;
    for (index, root) in roots.iter().enumerate() {
        let sibling_alias =
            root.with_file_name(root.file_name().unwrap().to_str().unwrap().to_uppercase());
        let candidates = [
            root.to_path_buf(),
            sibling_alias,
            ancestor_alias.join(root.strip_prefix(&fixture.root).unwrap()),
        ];
        for (kind, parent) in candidates.into_iter().enumerate() {
            let Ok(meta) = std::fs::metadata(&parent) else {
                continue;
            };
            let original = std::fs::metadata(root).unwrap();
            if kind > 0 && (meta.dev(), meta.ino()) != (original.dev(), original.ino()) {
                continue;
            }
            if kind > 0 {
                physical_aliases += 1;
            }
            let output = parent.join("must-not-exist");
            if index < 3 {
                assert!(plan::create(&fixture.config, &output).is_err());
            }
            assert!(
                stage::create(fixture.config.clone(), &plan_path, digest, &output)
                    .await
                    .is_err()
            );
            assert!(!output.exists());
            assert_eq!(snapshot(&roots), before);
        }
    }
    // Report platform coverage explicitly; the reviewer CLI probes additionally
    // require and demonstrate all case-folded aliases on the qualification host.
    eprintln!("physical alias refusals exercised: {physical_aliases}");
    let linked = fixture.root.join("linked-parent");
    symlink(&fixture.config.home, &linked).unwrap();
    assert!(stage::create(
        fixture.config.clone(),
        &plan_path,
        digest,
        &linked.join("must-not-exist")
    )
    .await
    .is_err());
    assert!(!fixture.config.home.join("must-not-exist").exists());
    assert_eq!(snapshot(&roots), before);
}

#[tokio::test]
async fn replaced_database_path_cannot_redirect_staging_writes_into_live_state() {
    let fixture = plans::fixture();
    let plan_path = fixture.root.join("plan");
    let planned = plan::create(&fixture.config, &plan_path).unwrap();
    let output = fixture.root.join("staged");
    let roots = [
        fixture.config.home.as_path(),
        fixture.config.state_root.as_path(),
        fixture.config.checkpoint_root.as_path(),
        &plan_path,
    ];
    let before = snapshot(&roots);
    let moved = fixture.root.join("detached-database");
    let result = stage::create_checked(
        fixture.config.clone(),
        &plan_path,
        planned["plan_sha256"].as_str().unwrap(),
        &output,
        stage::PAGES,
        |at| {
            if matches!(at, stage::Boundary::Database) {
                std::fs::rename(output.join("database"), &moved).unwrap();
                symlink(&fixture.config.state_root, output.join("database")).unwrap();
            }
            Ok(())
        },
    )
    .await;
    assert!(result.is_err());
    assert!(!output.join("ready.json").exists());
    assert_eq!(snapshot(&roots), before);
    let db = rusqlite::Connection::open_with_flags(
        moved.join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT admission FROM metadata", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "import_held"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM operations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn unavailable_physical_roots_refuse_output_creation() {
    let fixture = plans::fixture();
    let proof = isolation::Proof::new(&[fixture.config.home.as_path()]).unwrap();
    std::fs::rename(&fixture.config.home, fixture.root.join("moved-home")).unwrap();
    std::fs::create_dir(&fixture.config.home).unwrap();
    let output = fixture.root.join("must-not-exist");
    assert_eq!(
        proof.parent(&output).err().unwrap().code,
        "mutation_ledger_migration_changed"
    );
    std::fs::remove_dir(&fixture.config.home).unwrap();
    assert!(plan_io::Archive::create(&fixture.config, &output).is_err());
    assert!(!output.exists());
}
