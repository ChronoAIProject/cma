//! Disposable A/B interoperability fixture; its unit guard is not C authority.
use crate::config::RuntimeConfig;
use crate::ledger_reclamation::CollectionEvidence;
use crate::mutation_journal::Operation;
use crate::mutation_ledger::{
    retirement_inventory::{Context, SourceRow},
    Ledger,
};
use crate::retirement_archive::{self, Archive, CollectionReference, GroupProof, Version};
use layer3_adapter::runtime_ledger_collection::CollectionGroup;
use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;
use layer3_adapter::{AgentHomeRemoveRequest, AgentSlot};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(super) struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
    archive_id: String,
}

pub(super) struct Original {
    pub binding: LedgerBindingV1,
    pub bytes: [Vec<u8>; 3],
    pub proof: GroupProof,
    pub context: Context,
    rows: Vec<SourceRow>,
    pub operation: Operation,
}

impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-collector-storage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut config = RuntimeConfig::for_test(root.join("runtime"));
        config.home_identity = Some("a".repeat(43));
        Self {
            root,
            config,
            archive_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub async fn archive(&self, create: bool) -> Archive {
        Archive::open_version(
            self.root.join("archive"),
            self.config.clone(),
            self.archive_id.clone(),
            create,
            256,
            Version::Collection,
        )
        .await
        .unwrap()
    }

    pub async fn source(&self, create: bool) -> Ledger {
        Ledger::open(
            self.root.join("source"),
            self.config.clone(),
            create,
            16_384,
        )
        .await
        .unwrap()
    }

    pub async fn original(&self, name: &str, create: bool) -> Original {
        let source = self.source(create).await;
        let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
            format!("agent:{name}:home:remove:v1"),
            AgentSlot::new(name),
        ));
        source.admit(5, vec![operation.clone()]).await.unwrap();
        let entry = source.begin(operation.clone()).await.unwrap();
        source
            .finish(
                entry
                    .record
                    .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        agent_id: AgentSlot::new(name),
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut cursor = None;
        let mut context = None;
        let mut rows = Vec::new();
        loop {
            let page = source.retirement_inventory(cursor).await.unwrap();
            context.get_or_insert(page.context);
            rows.extend(page.rows);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        source.close().await.unwrap();
        let context = context.unwrap();
        let (binding, bytes) = retirement_archive::collection_probe::artifacts(
            &self.config,
            &self.archive_id,
            context.revision().unwrap(),
            CollectionGroup::Agent {
                agent_id: name.into(),
            },
        )
        .unwrap();
        let key = retirement_archive::group_key(&operation);
        let evidence = CollectionEvidence::parse(&binding, &key, bytes.clone()).unwrap();
        let mut hash = Sha256::new();
        context.hash_into(&mut hash);
        for row in &rows {
            row.hash_into(&mut hash);
        }
        let proof = GroupProof {
            key,
            source_sha256: format!("{:x}", hash.finalize()),
            observation_sha256: evidence.observation_sha256().into(),
            records: rows.len() as u32,
        };
        Original {
            binding,
            bytes,
            proof,
            context,
            rows,
            operation,
        }
    }

    pub async fn complete(&self, original: &Original, reference: CollectionReference) {
        let archive = self.archive(false).await;
        let rows = original.rows.clone();
        archive
            .prepare_indexed(original.proof.clone(), original.context.clone(), move |i| {
                Ok(rows[i as usize].clone())
            })
            .await
            .unwrap();
        // Only newly created local source/archive files. This test guard must
        // never be offered as physical exclusion or a qualified production permit.
        retirement_archive::complete_collection_held(
            archive,
            self.source(false).await,
            original.binding.clone(),
            reference,
            original.proof.clone(),
            (),
        )
        .await
        .unwrap();
    }
}

impl Original {
    pub fn evidence(&self) -> CollectionEvidence {
        CollectionEvidence::parse(&self.binding, &self.proof.key, self.bytes.clone()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
