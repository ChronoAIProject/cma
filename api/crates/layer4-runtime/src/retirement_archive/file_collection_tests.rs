use super::super::file_collection_transition::{complete_checked, Boundary};
use super::*;
use crate::ledger_reclamation::CollectionEvidence;
use layer3_adapter::runtime_ledger_collection::CollectionGroup;

struct Files {
    fixture: Fixture,
}
impl Files {
    fn new() -> Self {
        let mut fixture = Fixture::new();
        fixture.config = super::super::file_collection_probe::source(
            &fixture.root.join("files"),
            &fixture.config,
        )
        .unwrap();
        Self { fixture }
    }
    async fn open(&self, create: bool) -> Archive {
        Archive::open_version(
            self.fixture.path.clone(),
            self.fixture.config.clone(),
            self.fixture.id.clone(),
            create,
            32768,
            Version::FileCollection,
        )
        .await
        .unwrap()
    }
    fn add(&self, name: &str, complete: bool) -> Operation {
        super::super::file_collection_probe::add(&self.fixture.config, name, complete).unwrap()
    }
    async fn select(
        &self,
        archive: &Archive,
        name: &str,
    ) -> (
        layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1,
        CollectionReference,
        GroupProof,
    ) {
        self.select_group(
            archive,
            CollectionGroup::Agent {
                agent_id: name.into(),
            },
        )
        .await
    }
    async fn select_group(
        &self,
        archive: &Archive,
        group: CollectionGroup,
    ) -> (
        layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1,
        CollectionReference,
        GroupProof,
    ) {
        let revision = archive.observe_file_collection().await.unwrap();
        let key = group.key();
        let (binding, bytes) = super::super::collection_probe::file_artifacts(
            &self.fixture.config,
            &self.fixture.id,
            revision,
            group,
        )
        .unwrap();
        let evidence = CollectionEvidence::parse(&binding, &key, bytes).unwrap();
        let (reference, proof) = archive
            .prepare_file_collection(evidence, "a".repeat(64))
            .await
            .unwrap();
        (binding, reference, proof)
    }
    fn counts(&self) -> [usize; 2] {
        let root = &self.fixture.config.state_root;
        let mut scopes = vec![root.clone()];
        scopes.extend(
            std::fs::read_dir(root)
                .unwrap()
                .map(|p| p.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with("process-generation-")
                }),
        );
        ["runtime-mutations", "mutation-authority"].map(|name| {
            scopes
                .iter()
                .map(|root| {
                    std::fs::read_dir(root.join(name))
                        .map(|r| r.count())
                        .unwrap_or(0)
                })
                .sum()
        })
    }
}

#[tokio::test]
async fn file_completion_reconciles_every_durable_boundary_without_replacing_originals() {
    for failure in 0..5 {
        let files = Files::new();
        let live = files.add("unresolved", false);
        let retired = files.add("retired", true);
        let archive = files.open(true).await;
        let (binding, reference, proof) = files.select(&archive, "retired").await;
        let original = archive
            .read_collection(binding.clone(), reference.clone())
            .await
            .unwrap()
            .into_originals();
        assert!(archive.observe_file_collection().await.is_err());
        assert!(archive.require_ready().await.is_err());
        let mut hit = false;
        assert!(complete_checked(
            archive,
            binding.clone(),
            reference.clone(),
            proof.clone(),
            (),
            move |boundary| {
                let current = match boundary {
                    Boundary::BeforeFiles => 0,
                    Boundary::FileMoved => 1,
                    Boundary::FilesVerified => 2,
                    Boundary::Acknowledged => 3,
                    Boundary::Closed => 4,
                };
                if current == failure && !hit {
                    hit = true;
                    Err(invalid())
                } else {
                    Ok(())
                }
            }
        )
        .await
        .is_err());
        let archive = files.open(false).await;
        assert_eq!(
            archive
                .read_collection(binding.clone(), reference.clone())
                .await
                .unwrap()
                .originals(),
            original.each_ref().map(Vec::as_slice)
        );
        let receipt = complete_file_collection_held(
            archive,
            binding.clone(),
            reference.clone(),
            proof.clone(),
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            (receipt.operations, receipt.authority, receipt.cleanup),
            (1, 1, 0)
        );
        assert_eq!(files.counts(), [1, 1]);
        let archive = files.open(false).await;
        archive.require_ready().await.unwrap();
        assert!(archive.authorize(&[retired]).await.is_err());
        archive.authorize(&[live]).await.unwrap();
        assert_eq!(
            complete_file_collection_held(archive, binding, reference, proof, ())
                .await
                .unwrap(),
            receipt
        );
    }
}

#[tokio::test]
async fn file_completion_refuses_unrelated_drift_missing_copies_and_changed_references() {
    for failure in 0..3 {
        let files = Files::new();
        files.add("selected", true);
        let archive = files.open(true).await;
        let (binding, mut reference, proof) = files.select(&archive, "selected").await;
        match failure {
            0 => {
                files.add("unexpected", true);
            }
            1 => {
                let path =
                    std::fs::read_dir(files.fixture.config.state_root.join("runtime-mutations"))
                        .unwrap()
                        .next()
                        .unwrap()
                        .unwrap()
                        .path();
                std::fs::remove_file(path).unwrap();
            }
            _ => reference.artifact_lengths[0] += 1,
        }
        assert!(
            complete_file_collection_held(archive, binding, reference, proof, ())
                .await
                .is_err()
        );
        let archive = files.open(false).await;
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
        assert_eq!(files.counts()[1], if failure == 0 { 2 } else { 1 });
    }
}

#[tokio::test]
async fn file_completion_keeps_unsettled_selection_held_and_refuses_format_confusion() {
    let files = Files::new();
    files.add("unsettled", false);
    let archive = files.open(true).await;
    let revision = archive.observe_file_collection().await.unwrap();
    let (binding, bytes) = super::super::collection_probe::file_artifacts(
        &files.fixture.config,
        &files.fixture.id,
        revision,
        CollectionGroup::Agent {
            agent_id: "unsettled".into(),
        },
    )
    .unwrap();
    let key = layer3_adapter::runtime_ledger_retirement::ledger_retired_agent_key("unsettled");
    let evidence = CollectionEvidence::parse(&binding, &key, bytes).unwrap();
    assert!(archive
        .prepare_file_collection(evidence, "a".repeat(64))
        .await
        .is_err());
    let mut reference = archive.pending_collection(binding).await.unwrap().unwrap();
    assert!(reference.validate());
    reference.archive_version = 3;
    assert!(!reference.validate());
    reference.archive_version = 4;
    reference.format = layer3_adapter::LEDGER_COLLECTION_ARTIFACT_FORMAT.into();
    assert!(!reference.validate());
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
    assert!(Archive::open_version(
        files.fixture.path.clone(),
        files.fixture.config.clone(),
        files.fixture.id.clone(),
        false,
        32768,
        Version::Collection
    )
    .await
    .is_err());
    assert_eq!(files.counts(), [1, 1]);
}

#[path = "file_collection_crash_tests.rs"]
mod crashes;
#[path = "file_collection_lifetime_tests.rs"]
mod lifetimes;
