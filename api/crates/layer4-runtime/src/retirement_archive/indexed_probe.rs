//! The shipped executable qualifies held V2 copy/reopen and reader refusal on
//! create-only synthetic source state. No retained home is opened or changed.
use super::*;
use crate::mutation_ledger::Ledger;

pub(super) async fn run(path: &Path) -> Result<()> {
    let config = RuntimeConfig::from_values(BTreeMap::from([
        (
            "CHRONO_SANDBOX_ID".into(),
            "indexed-retirement-probe".into(),
        ),
        (
            "HOME".into(),
            path.with_extension("home").to_string_lossy().into(),
        ),
        (
            "CHRONO_RUNTIME_STATE_ROOT".into(),
            path.with_extension("state").to_string_lossy().into(),
        ),
        ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
        (
            "TMPDIR".into(),
            path.with_extension("transfer").to_string_lossy().into(),
        ),
    ]))
    .map_err(|_| invalid())?;
    let id = uuid::Uuid::new_v4().to_string();
    // Reuse the synthetic pair: another two stores would exceed the image
    // probe's 128 MiB filesystem once their emergency reserves were allocated.
    for collected in [false, true] {
        qualify(path, &config, &id, collected).await?;
    }
    // Both owners are closed. Reuse this create-only synthetic allocation for
    // the new format without increasing the 128 MiB image probe footprint.
    std::fs::remove_dir_all(path).map_err(|_| invalid())?;
    std::fs::remove_dir_all(path.with_extension("source")).map_err(|_| invalid())?;
    super::super::collection_storage_probe::run(path, &config).await
}

async fn qualify(path: &Path, config: &RuntimeConfig, id: &str, collected: bool) -> Result<()> {
    let ledger = Ledger::open(
        path.with_extension("source"),
        config.clone(),
        !collected,
        16_384,
    )
    .await?;
    let name = if collected {
        "indexed-collection-probe"
    } else {
        "indexed-probe"
    };
    let operation = Operation::remove_agent_home(&AgentHomeRemoveRequest::new(
        format!("agent:{name}:home:remove:v1"),
        AgentSlot::new(name),
    ));
    ledger.admit(5, vec![operation.clone()]).await?;
    let entry = ledger.begin(operation.clone()).await?;
    ledger
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
    let before = ledger.snapshot().await?;
    let mut cursor = None;
    let mut context = None;
    let mut rows = Vec::new();
    loop {
        let page = ledger.retirement_inventory(cursor).await?;
        context.get_or_insert(page.context);
        rows.extend(page.rows);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    let context = context.ok_or_else(invalid)?;
    let mut digest = Sha256::new();
    context.hash_into(&mut digest);
    for row in &rows {
        row.hash_into(&mut digest);
    }
    let mut proof = GroupProof {
        key: group_key(&operation),
        source_sha256: format!("{:x}", digest.finalize()),
        observation_sha256: format!("{:x}", Sha256::digest(observation(config, &operation)?)),
        records: rows.len() as u32,
    };
    let (binding, originals) = super::super::collection_probe::artifacts(
        config,
        id,
        context.revision()?,
        layer3_adapter::runtime_ledger_collection::CollectionGroup::Agent {
            agent_id: name.into(),
        },
    )?;
    let collection = || {
        crate::ledger_reclamation::CollectionEvidence::parse(
            &binding,
            &group_key(&operation),
            originals.clone(),
        )
    };
    if collected {
        let evidence = collection()?;
        if evidence.originals() != originals.each_ref().map(Vec::as_slice)
            || evidence.operation_id().is_empty()
        {
            return Err(invalid());
        }
        proof.observation_sha256 = evidence.observation_sha256().into();
    }
    let archive = Archive::open_version(
        path.to_owned(),
        config.clone(),
        id.into(),
        !collected,
        256,
        Version::Indexed,
    )
    .await?;
    let receipt = archive
        .prepare_indexed(proof.clone(), context.clone(), move |i| {
            Ok(rows[i as usize].clone())
        })
        .await?;
    if receipt.complete
        || archive.require_ready().await.is_ok()
        || ledger.snapshot().await? != before
    {
        return Err(invalid());
    }
    expect_refused(&archive, &operation).await?;
    archive.close().await?;
    // This uses the exact V1 schema expectation, which must not reinterpret V2.
    if let Ok(old) = Archive::open(path.to_owned(), config.clone(), id.into(), false, 256).await {
        old.close().await?;
        return Err(invalid());
    }
    let archive = Archive::open_version(
        path.to_owned(),
        config.clone(),
        id.into(),
        false,
        256,
        Version::Indexed,
    )
    .await?;
    let selected = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = selected.clone();
    let expected = context.clone();
    archive
        .visit_indexed_sources(proof.clone(), move |context, row| {
            let mut rows = captured.lock().map_err(|_| invalid())?;
            // This create-only fixture has one resource and fits one page.
            if context != &expected || rows.len() == 64 {
                return Err(invalid());
            }
            rows.push(row);
            Ok(())
        })
        .await?;
    let rows = std::mem::take(&mut *selected.lock().map_err(|_| invalid())?);
    ledger
        .verify_retirement_sources(context.clone(), rows)
        .await?;
    if archive.verify_indexed(proof.clone()).await? != receipt
        || archive
            .prepare_indexed(proof.clone(), context, |_| Err(invalid()))
            .await?
            != receipt
    {
        return Err(invalid());
    }
    expect_refused(&archive, &operation).await?;
    let observation = observation(config, &operation)?;
    let first = if collected {
        super::super::deactivate_collected_held(archive, ledger, proof.clone(), collection()?, ())
            .await?
    } else {
        super::super::deactivate_held(archive, ledger, proof.clone(), observation.clone(), ())
            .await?
    };
    let archive = Archive::open_version(
        path.to_owned(),
        config.clone(),
        id.into(),
        false,
        256,
        Version::Indexed,
    )
    .await?;
    let ledger = Ledger::open(path.with_extension("source"), config.clone(), false, 16_384).await?;
    let after = ledger.snapshot().await?;
    if after.operations != 0
        || after.authority != 0
        || after.terminal != 0
        || after.reserved_bytes != 0
        || archive.require_ready().await.is_ok()
    {
        return Err(invalid());
    }
    expect_refused(&archive, &operation).await?;
    let retry = if collected {
        super::super::deactivate_collected_held(archive, ledger, proof, collection()?, ()).await?
    } else {
        super::super::deactivate_held(archive, ledger, proof, observation, ()).await?
    };
    if retry != first {
        return Err(invalid());
    }
    Ok(())
}

fn observation(config: &RuntimeConfig, operation: &Operation) -> Result<Vec<u8>> {
    use layer3_adapter::runtime_ledger_retirement::{
        RuntimeLedgerRetirementSnapshot, LEDGER_RETIREMENT_SNAPSHOT_FORMAT,
    };
    serde_json::to_vec(&RuntimeLedgerRetirementSnapshot {
        format: LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
        sandbox_id: config.sandbox_id.clone(),
        home_identity: config.home_identity.clone().ok_or_else(invalid)?,
        generation: config.generation,
        sandbox_revision: 1,
        observed_at: "2026-11-11T00:00:00Z".into(),
        source_sha256: "b".repeat(64),
        agents_observed: 1,
        worktrees_observed: 0,
        deleted_agents: vec![group_key(operation)],
        removed_worktrees: vec![],
    })
    .map_err(|_| invalid())
}
