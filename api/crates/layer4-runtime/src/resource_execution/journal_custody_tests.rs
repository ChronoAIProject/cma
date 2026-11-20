use super::*;

fn replace_file(path: &std::path::Path) {
    let retained = path.with_extension("retained");
    std::fs::rename(path, &retained).unwrap();
    std::fs::copy(&retained, path).unwrap();
}

fn replace_directory(root: &std::path::Path) {
    let retained = root.with_extension("retained");
    std::fs::rename(root, &retained).unwrap();
    std::fs::create_dir(root).unwrap();
    for name in ["writer.lock", "state.json"] {
        std::fs::copy(retained.join(name), root.join(name)).unwrap();
    }
}

#[tokio::test]
async fn same_content_journal_identity_replacement_closes_original_receiver() {
    for replacement in ["writer", "marker", "directory", "parent"] {
        let f = Fixture::new().await;
        let _dispatch = f.dispatch("command").await;
        let gate = f.receiver.state.lock().unwrap().gates[0].clone();
        let root = f.root.join("receiver");
        match replacement {
            "writer" => replace_file(&root.join("writer.lock")),
            "marker" => replace_file(&root.with_extension("activated")),
            "directory" => replace_directory(&root),
            "parent" => {
                let retained = f.root.with_extension("retained");
                std::fs::rename(&f.root, &retained).unwrap();
                std::fs::create_dir_all(&root).unwrap();
                for name in ["writer.lock", "state.json"] {
                    std::fs::copy(retained.join("receiver").join(name), root.join(name)).unwrap();
                }
                std::fs::copy(
                    retained.join("receiver.activated"),
                    root.with_extension("activated"),
                )
                .unwrap();
                std::fs::remove_dir_all(retained).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(f.receiver.state().is_err(), "replacement: {replacement}");
        assert!(gate.ready_to_observe_stop());
        assert!(!f.receiver.legacy_allowed());
        assert!(f.receiver.capabilities().is_none());
    }
}

#[tokio::test]
async fn replacing_one_journal_identity_cannot_admit_a_second_writer() {
    for replacement in ["writer", "marker", "directory"] {
        let f = Fixture::new().await;
        let root = f.root.join("receiver");
        match replacement {
            "writer" => replace_file(&root.join("writer.lock")),
            "marker" => replace_file(&root.with_extension("activated")),
            "directory" => replace_directory(&root),
            _ => unreachable!(),
        }
        let second = Receiver::with_runner(
            root,
            f.receiver.identity.clone(),
            true,
            f.runner.clone(),
            f.clock.clone(),
        );
        assert!(second.is_err(), "replacement: {replacement}");
    }
}

#[tokio::test]
async fn removing_writer_custody_closes_existing_gates() {
    let f = Fixture::new().await;
    let _dispatch = f.dispatch("command").await;
    let gate = f.receiver.state.lock().unwrap().gates[0].clone();
    std::fs::remove_file(f.root.join("receiver/writer.lock")).unwrap();
    assert!(f.receiver.state().is_err());
    assert!(gate.ready_to_observe_stop());
    assert!(!f.receiver.legacy_allowed());
}
