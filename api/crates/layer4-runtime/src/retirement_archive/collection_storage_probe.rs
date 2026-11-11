//! Create-only synthetic qualification of original-byte retention and cold
//! completion. It supplies no real-home exclusion or activation authority.
use std::path::Path;

use layer3_adapter::runtime_ledger_collection::CollectionGroup;
use layer3_adapter::{AgentHomeRemoveRequest, AgentSlot};
use sha2::{Digest, Sha256};

use super::{invalid, Archive, GroupProof, Version};
use crate::mutation_journal::Operation;
use crate::mutation_ledger::Ledger;
use crate::{config::RuntimeConfig, failure::Result, ledger_reclamation::CollectionEvidence};

pub(super) async fn run(path: &Path, config: &RuntimeConfig) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let mut original = None;
    for (index, name) in ["collection-first", "collection-later"]
        .into_iter()
        .enumerate()
    {
        let source = Ledger::open(
            path.with_extension("source"),
            config.clone(),
            index == 0,
            16_384,
        )
        .await?;
        let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
            format!("agent:{name}:home:remove:v1"),
            AgentSlot::new(name),
        ));
        source.admit(5, vec![operation.clone()]).await?;
        let entry = source.begin(operation.clone()).await?;
        source
            .finish(
                entry
                    .record
                    .completed(&layer3_adapter::wire::AgentHomeRemoveReceipt {
                        operation_id: operation.operation_id.clone(),
                        request_sha256: operation.request_sha256.clone(),
                        agent_id: AgentSlot::new(name),
                    })?,
            )
            .await?;
        let mut cursor = None;
        let mut context = None;
        let mut rows = Vec::new();
        loop {
            let page = source.retirement_inventory(cursor).await?;
            context.get_or_insert(page.context);
            rows.extend(page.rows);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        let context = context.ok_or_else(invalid)?;
        let key = super::group_key(&operation);
        let (binding, bytes) = super::collection_probe::artifacts(
            config,
            &id,
            context.revision()?,
            CollectionGroup::Agent {
                agent_id: name.into(),
            },
        )?;
        let evidence = CollectionEvidence::parse(&binding, &key, bytes.clone())?;
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
        let archive = Archive::open_version(
            path.into(),
            config.clone(),
            id.clone(),
            index == 0,
            256,
            Version::Collection,
        )
        .await?;
        let reference = archive.retain_collection(evidence, "a".repeat(64)).await?;
        if archive.pending_collection(binding.clone()).await? != Some(reference.clone())
            || archive.require_ready().await.is_ok()
            || archive
                .read_collection(binding.clone(), reference.clone())
                .await?
                .originals()
                != bytes.each_ref().map(Vec::as_slice)
        {
            return Err(invalid());
        }
        archive
            .prepare_indexed(
                proof.clone(),
                context,
                move |i| Ok(rows[i as usize].clone()),
            )
            .await?;
        let completed = super::complete_collection_held(
            archive,
            source,
            binding.clone(),
            reference.clone(),
            proof.clone(),
            (),
        )
        .await?;
        if index == 0 {
            original = Some((binding, reference, proof, completed));
        }
        let archive = Archive::open_version(
            path.into(),
            config.clone(),
            id.clone(),
            false,
            256,
            Version::Collection,
        )
        .await?;
        archive.require_ready().await?;
        if archive.authorize(&[operation]).await.is_ok() {
            return Err(invalid());
        }
        archive.close().await?;
    }
    // Retry the first operation after a later group's source revision and cold
    // acknowledgement. It must return exactly the original receipt.
    let (binding, reference, proof, expected) = original.ok_or_else(invalid)?;
    let source = Ledger::open(path.with_extension("source"), config.clone(), false, 16_384).await?;
    let archive = Archive::open_version(
        path.into(),
        config.clone(),
        id.clone(),
        false,
        256,
        Version::Collection,
    )
    .await?;
    if super::complete_collection_held(archive, source, binding, reference, proof, ()).await?
        != expected
    {
        return Err(invalid());
    }
    if let Ok(old) = Archive::open_version(
        path.into(),
        config.clone(),
        id,
        false,
        256,
        Version::Indexed,
    )
    .await
    {
        old.close().await?;
        return Err(invalid());
    }
    Ok(())
}
