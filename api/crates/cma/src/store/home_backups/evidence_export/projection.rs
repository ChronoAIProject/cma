use super::{digest, Result};
use mongodb::bson::{Bson, Document};
use serde_json::{json, Value};

const MAX_SAFE_INTEGER: i64 = (1_i64 << 53) - 1;

fn exact_numbers(value: &Bson) -> Result<()> {
    match value {
        Bson::Int64(n) if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(n) => {
            return Err("catalog_export_numeric_range");
        }
        Bson::Document(row) => {
            for value in row.values() {
                exact_numbers(value)?;
            }
        }
        Bson::Array(values) => {
            for value in values {
                exact_numbers(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(in crate::store::home_backups) fn project(collection: &str, row: Document) -> Result<Value> {
    exact_numbers(&Bson::Document(row.clone()))?;
    let invalid = |_| "catalog_export_invalid_record";
    match collection {
        "sandboxes" => {
            let value = crate::store::sandboxes::read_sandbox(&row)
                .map_err(|_| "catalog_export_invalid_record")?;
            let home = value.lifecycle.home.as_ref().map(|home| {
                let receipt: Result<Option<Value>> = home.receipt.as_ref().map(|receipt| {
                    let attachment = layer3_adapter::HomeVolumeAttachment {
                        request: home.request.clone(), receipt: receipt.clone(),
                    };
                    Ok(json!({
                        "storage_generation": receipt.storage_generation,
                        "home_identity": attachment.identity().map_err(|_| "catalog_export_invalid_record")?,
                        "storage_reference": receipt.storage_reference,
                    }))
                }).transpose();
                receipt.map(|receipt| json!({"released": home.released, "receipt": receipt}))
            }).transpose()?;
            Ok(
                json!({"id": value.id.as_str(), "owner_sha256": digest(value.owner.as_str().as_bytes()), "home": home}),
            )
        }
        super::SCHEDULES_COLLECTION => {
            let value = super::super::codec::read_schedule(row).map_err(invalid)?;
            Ok(json!({
                "id": value.id, "home": value.home, "state": value.state,
                "next_due_at": value.next_due_at, "last_ready_at": value.last_ready_at,
                "last_ready_backup_id": value.last_ready_backup_id, "in_flight": value.in_flight,
                "revision": value.revision, "next_fence": value.next_fence,
                "authority": {"validation": "strict_codec_v1", "lease_present": value.lease.is_some()},
            }))
        }
        super::BACKUPS_COLLECTION => {
            let value = super::super::codec::read_backup(row).map_err(invalid)?;
            Ok(json!({
                "id": value.id, "sandbox_id": value.sandbox_id, "epoch": value.epoch,
                "source": value.source, "cut": value.cut, "storage": value.storage,
                "custody": value.custody, "retention": value.retention, "state": value.state,
                "restore_pins": value.restore_pins, "next_fence": value.next_fence,
                "revision": value.revision, "failure": value.failure, "erasure": value.erasure,
                "authority": {"validation": "strict_codec_v1", "lease_present": value.lease.is_some()},
            }))
        }
        _ => Err("catalog_export_invalid_scope"),
    }
}
