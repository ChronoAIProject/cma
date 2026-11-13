use super::*;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use crate::mutation_authority::Record as Authority;
use crate::mutation_journal::Record as Journal;
use crate::retirement_archive::{group_key, GroupProof, SourceRecord};
use layer3_adapter::{AgentHomeRemoveRequest, AgentSlot};
use sha2::{Digest, Sha256};

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) config: RuntimeConfig,
    pub(super) id: String,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-retirement-reader-{}", uuid::Uuid::new_v4()));
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.home_identity = Some("a".repeat(43));
        config.state_root = root.join("state");
        config.checkpoint_root = root.join("transfer/cma-checkpoints");
        config.process_env.insert(
            "TMPDIR".into(),
            root.join("transfer").to_str().unwrap().into(),
        );
        for path in [
            config.home.clone(),
            config.state_root.clone(),
            config.checkpoint_root.join("runtime-state"),
        ] {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .unwrap();
        }
        Self {
            root,
            config,
            id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub(super) async fn ledger(
        &self,
        indexed: bool,
    ) -> Option<Arc<crate::mutation_ledger::Ledger>> {
        let selection = indexed.then(|| crate::ledger_format::select(&self.config).unwrap());
        crate::persistent_home::initialize(&self.config).unwrap();
        match selection {
            Some(selection) => selection.open(&self.config).await.unwrap(),
            None => None,
        }
    }

    pub(super) fn descriptor(&self) -> Descriptor {
        Descriptor {
            version: 1,
            archive: self.id.clone(),
            sandbox: self.config.sandbox_id.clone(),
            home: self.config.home_identity.clone().unwrap(),
            state_root: self.config.state_root.clone(),
            home_root: self.config.home.clone(),
            transfer_root: self.config.checkpoint_root.clone(),
            born: self.config.generation,
            pages: PAGES,
            active_v2_sha256: descriptor::active_format(&self.config).unwrap(),
        }
    }

    pub(super) fn publish(&self, descriptor: &Descriptor) {
        let bytes = serde_json::to_vec(descriptor).unwrap();
        write(&self.config.state_root.join(MANIFEST), &bytes);
        write(&descriptor.marker(), &bytes);
    }

    pub(super) async fn activate(&self, complete: bool) -> GroupProof {
        let archive = Archive::open(
            self.config.state_root.join(DATABASE),
            self.config.clone(),
            self.id.clone(),
            true,
            PAGES,
        )
        .await
        .unwrap();
        let op = operation("retired");
        let journal = Journal::started(&self.config, &op)
            .unwrap()
            .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                operation_id: op.operation_id.clone(),
                request_sha256: op.request_sha256.clone(),
                agent_id: AgentSlot::new("retired"),
            })
            .unwrap();
        let authority = Authority::new(&self.config, 4, &op);
        let name = |path: PathBuf| path.file_name().unwrap().to_str().unwrap().to_owned();
        let records = [
            SourceRecord {
                scope: 0,
                kind: 1,
                name: name(journal.inventory_name(&self.config)),
                bytes: journal.encode().unwrap(),
            },
            SourceRecord {
                scope: 0,
                kind: 2,
                name: name(authority.inventory_name(&self.config)),
                bytes: serde_json::to_vec(&authority).unwrap(),
            },
        ];
        let mut digest = Sha256::new();
        for record in &records {
            record.hash_into(&mut digest);
        }
        let proof = GroupProof {
            key: group_key(&op),
            source_sha256: format!("{:x}", digest.finalize()),
            observation_sha256: "b".repeat(64),
            records: 2,
        };
        archive
            .prepare(proof.clone(), move |index| {
                Ok(records[index as usize].clone())
            })
            .await
            .unwrap();
        if complete {
            archive
                .complete(proof.key.clone(), proof.source_sha256.clone())
                .await
                .unwrap();
        }
        archive.close().await.unwrap();
        self.publish(&self.descriptor());
        proof
    }

    pub(super) async fn reader(&self) -> Reader {
        select(&self.config)
            .unwrap()
            .open(&self.config)
            .await
            .unwrap()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

pub(super) fn write(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

pub(super) fn operation(agent: &str) -> Operation {
    Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        format!("agent:{agent}:home:remove:v1"),
        AgentSlot::new(agent),
    ))
}
