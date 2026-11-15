//! Retain a bounded, allowlisted close summary, never arbitrary child output.
use super::cleanup::Failure;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
use tokio::time::Instant;

pub(super) async fn read_closed(
    output: &mut (impl AsyncBufRead + Unpin),
    deadline: Instant,
) -> Result<Value, Failure> {
    let mut line = String::new();
    let mut bounded = output.take(2 * 1024 * 1024);
    tokio::time::timeout_at(deadline, bounded.read_line(&mut line))
        .await
        .map_err(|_| Failure::ReceiptTimeout)?
        .map_err(|_| Failure::ReceiptRead)?;
    if !line.ends_with('\n') {
        return Err(Failure::ReceiptInvalid);
    }
    let row: Value = serde_json::from_str(&line).map_err(|_| Failure::ReceiptInvalid)?;
    if row["type"] != "closed" || row["reason"] != "stdin_closed" {
        return Err(Failure::ReceiptInvalid);
    }
    let receipt = &row["receipt"];
    if receipt["schema"] != "ordinary-work-provider-ledger-v1"
        || receipt["failed"] != false
        || receipt.get("attempted_sends") != Some(&Value::Null)
    {
        return Err(Failure::ReceiptInvalid);
    }
    let mut summary = json!({"schema": "provider-test-close-summary-v1", "attempted_sends": null});
    for field in [
        "received_mint_posts",
        "accepted_calls",
        "distinct_effects",
        "physical_commits",
        "replays",
        "refusals",
    ] {
        summary[field] = json!(receipt[field].as_u64().ok_or(Failure::ReceiptInvalid)?);
    }
    let pending = receipt["pending_at_close"]
        .as_array()
        .ok_or(Failure::ReceiptInvalid)?;
    if pending.len() > 32 {
        return Err(Failure::ReceiptInvalid);
    }
    let mut retained = Vec::new();
    for row in pending {
        let request = row["request"]
            .as_u64()
            .filter(|id| *id > 0)
            .ok_or(Failure::ReceiptInvalid)?;
        let phase = row["phase"].as_str().ok_or(Failure::ReceiptInvalid)?;
        if !["preparation_held", "acceptance_held", "response_held"].contains(&phase) {
            return Err(Failure::ReceiptInvalid);
        }
        retained.push(json!({"request": request, "phase": phase}));
    }
    summary["pending_at_close"] = json!(retained);
    summary["wire_sha256"] = json!(ring::digest::digest(&ring::digest::SHA256, line.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>());
    // Counts are receiver observations, not a send ledger or a claim of effect
    // finality. Successful tests print this summary before deleting local state.
    Ok(summary)
}
