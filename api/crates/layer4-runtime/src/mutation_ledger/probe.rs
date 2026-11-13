use super::*;
use std::path::Path;

pub(crate) async fn run(directory: &Path) -> Result<()> {
    let config = RuntimeConfig::from_values(std::collections::BTreeMap::from([
        ("CHRONO_SANDBOX_ID".into(), "ledger-storage-probe".into()),
        (
            "HOME".into(),
            directory.with_extension("home").to_string_lossy().into(),
        ),
        (
            "CHRONO_RUNTIME_STATE_ROOT".into(),
            directory.to_string_lossy().into(),
        ),
    ]))
    .map_err(|_| invalid())?;
    let operation = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:storage-probe:close:v1",
        layer3_adapter::AgentSlot::new("storage-probe"),
    ));
    let ledger = Ledger::open(directory.to_owned(), config.clone(), true, 16_384).await?;
    ledger.admit(u64::MAX, vec![operation.clone()]).await?;
    let entry = ledger.begin(operation.clone()).await?;
    if !entry.new {
        return Err(invalid());
    }
    if !ledger.admitted(operation.clone()).await?
        || ledger.recovery_fence(operation.clone()).await? != Some(u64::MAX)
        || ledger.started_page(0, String::new()).await?.len() != 1
    {
        return Err(invalid());
    }
    for filter in [
        ProcessFilter::Any,
        ProcessFilter::Agent("storage-probe".into()),
        ProcessFilter::Worktree("/absent/worktree".into()),
        ProcessFilter::Workspace("/absent".into()),
    ] {
        if ledger.active(filter).await? {
            return Err(invalid());
        }
    }
    let receipt = layer3_adapter::wire::AgentCloseReceipt {
        operation_id: operation.operation_id.clone(),
        request_sha256: operation.request_sha256.clone(),
        agent_id: layer3_adapter::AgentSlot::new("storage-probe"),
    };
    ledger.finish(entry.record.completed(&receipt)?).await?;
    ledger.close().await?;
    let ledger = Ledger::open(directory.to_owned(), config, false, 16_384).await?;
    ledger.admit(u64::MAX, vec![operation.clone()]).await?;
    let replay = ledger.begin(operation.clone()).await?;
    if replay.new
        || replay
            .record
            .receipt::<layer3_adapter::wire::AgentCloseReceipt>()?
            != receipt
    {
        return Err(invalid());
    }
    let failure_operation = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:failure-probe:close:v1",
        layer3_adapter::AgentSlot::new("failure-probe"),
    ));
    ledger.admit(1, vec![failure_operation.clone()]).await?;
    let entry = ledger.begin(failure_operation.clone()).await?;
    ledger
        .finish(entry.record.failed(
            &RuntimeError::rejected("probe_failure", "synthetic failure"),
            false,
        )?)
        .await?;
    if ledger.begin(failure_operation).await?.record.error()?.code != "probe_failure" {
        return Err(invalid());
    }
    let error = ledger
        .admit(u64::MAX - 1, vec![operation])
        .await
        .err()
        .ok_or_else(invalid)?;
    if error.code != "mutation_authority_stale" {
        return Err(invalid());
    }
    if !ledger.started_page(0, String::new()).await?.is_empty() {
        return Err(invalid());
    }
    let snapshot = ledger.snapshot().await?;
    if snapshot.operations != 2
        || snapshot.terminal != 2
        || snapshot.pending != 0
        || snapshot.processes != 0
        || snapshot.reserved_bytes != 0
    {
        return Err(invalid());
    }
    use sha2::{Digest, Sha256};
    let mut previous = None;
    for _ in 0..2 {
        let mut cursor = None;
        let mut rows = 0;
        let mut digest = Sha256::new();
        loop {
            let page = ledger.retirement_inventory(cursor).await?;
            if rows == 0 {
                page.context.hash_into(&mut digest);
            }
            for row in &page.rows {
                row.hash_into(&mut digest);
                rows += 1;
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        let hash = digest.finalize();
        if rows != 6 || previous.as_ref().is_some_and(|prior| prior != &hash) {
            return Err(invalid());
        }
        previous = Some(hash);
    }
    if ledger.snapshot().await? != snapshot {
        return Err(invalid());
    }
    ledger.close().await
}
