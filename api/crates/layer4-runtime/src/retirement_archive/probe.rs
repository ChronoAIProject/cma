//! Synthetic, create-only exact-image/storage qualification. No retained source
//! is opened or admitted. Only create-only synthetic sources are moved/restored.
use std::collections::BTreeMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use super::*;
use crate::mutation_authority::Record as Authority;
use crate::mutation_journal::Record as Journal;
use layer3_adapter::{AgentHomeRemoveRequest, AgentSlot};
#[path = "indexed_probe.rs"]
mod indexed;

pub(crate) async fn run(directory: &Path) -> Result<()> {
    for anchored in [false, true] {
        let path = if anchored {
            directory.with_extension("retirement-anchored")
        } else {
            directory.with_extension("retirement")
        };
        qualify(&path, anchored).await?;
    }
    indexed::run(&directory.with_extension("retirement-indexed"), false).await?;
    indexed::run(&directory.with_extension("retirement-collected"), true).await
}

async fn qualify(directory: &Path, anchored: bool) -> Result<()> {
    let config = RuntimeConfig::from_values(BTreeMap::from([
        (
            "CHRONO_SANDBOX_ID".into(),
            "retirement-storage-probe".into(),
        ),
        (
            "HOME".into(),
            directory.with_extension("home").to_string_lossy().into(),
        ),
        (
            "CHRONO_RUNTIME_STATE_ROOT".into(),
            directory.with_extension("state").to_string_lossy().into(),
        ),
        ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
        (
            "TMPDIR".into(),
            directory
                .with_extension("transfer")
                .to_string_lossy()
                .into(),
        ),
    ]))
    .map_err(|_| invalid())?;
    let id = uuid::Uuid::new_v4().to_string();
    let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        "agent:retirement-probe:home:remove:v1",
        AgentSlot::new("retirement-probe"),
    ));
    let journal = Journal::started(&config, &operation)?.completed(
        &layer3_adapter::wire::AgentHomeRemoveReceipt {
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            agent_id: AgentSlot::new("retirement-probe"),
        },
    )?;
    let authority = Authority::new(&config, u64::MAX, &operation);
    let name = |path: PathBuf| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(invalid)
    };
    let records = [
        SourceRecord {
            scope: 0,
            kind: 1,
            name: name(journal.inventory_name(&config))?,
            bytes: journal.encode()?,
        },
        SourceRecord {
            scope: 0,
            kind: 2,
            name: name(authority.inventory_name(&config))?,
            bytes: serde_json::to_vec(&authority).map_err(|_| invalid())?,
        },
    ];
    let sources = Arc::new(super::probe_sources::Sources::create(directory, &records)?);
    let mut digest = Sha256::new();
    for record in &records {
        record.hash_into(&mut digest);
    }
    let proof = GroupProof {
        key: group_key(&operation),
        source_sha256: format!("{:x}", digest.finalize()),
        observation_sha256: "b".repeat(64),
        records: 2,
    };
    let archive = if anchored {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|_| storage_error())?;
        let descriptor = Arc::new(std::fs::File::open(directory).map_err(|_| storage_error())?);
        Archive::create_anchored(descriptor, config.clone(), id.clone(), 256).await?
    } else {
        Archive::open(directory.to_owned(), config.clone(), id.clone(), true, 256).await?
    };
    let result = async {
        archive.authorize(std::slice::from_ref(&operation)).await?;
        let receipt = archive
            .prepare(proof.clone(), move |index| {
                Ok(records[index as usize].clone())
            })
            .await?;
        if receipt.records != 2 || receipt.complete {
            return Err(invalid());
        }
        expect_refused(&archive, &operation).await
    }
    .await;
    let closed = archive.close().await;
    result?;
    closed?;
    let archive = Archive::open(directory.to_owned(), config, id, false, 256).await?;
    let result = async {
        expect_refused(&archive, &operation).await?;
        let selected = sources.clone();
        archive
            .transfer_group(proof.clone(), Direction::Deactivate, move |record| {
                selected.transfer(record, false)
            })
            .await?;
        if sources.counts()? != (0, 2) {
            return Err(invalid());
        }
        archive.require_ready().await?;
        let selected = sources.clone();
        let restored = archive
            .transfer_group(proof.clone(), Direction::RestoreHeld, move |record| {
                selected.transfer(record, true)
            })
            .await?;
        if restored.complete || sources.counts()? != (2, 0) || archive.require_ready().await.is_ok()
        {
            return Err(invalid());
        }
        expect_refused(&archive, &operation).await?;
        let selected = sources.clone();
        archive
            .transfer_group(proof.clone(), Direction::Deactivate, move |record| {
                selected.transfer(record, false)
            })
            .await?;
        let repeated = archive.prepare(proof, |_| Err(invalid())).await?;
        if !repeated.complete || repeated.records != 2 {
            return Err(invalid());
        }
        expect_refused(&archive, &operation).await
    }
    .await;
    let closed = archive.close().await;
    result?;
    closed
}

async fn expect_refused(archive: &Archive, operation: &Operation) -> Result<()> {
    match archive.authorize(std::slice::from_ref(operation)).await {
        Err(error) if error.code == "mutation_resource_retired" => Ok(()),
        Err(error) => Err(error),
        Ok(()) => Err(invalid()),
    }
}
